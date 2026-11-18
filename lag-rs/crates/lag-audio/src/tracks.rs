//! Mirrors VfrReplayEngine.cs StartAudio/PumpAudio and AudioTrackState (rings stay engine-owned).
use crate::{AacAudioEncoder, EncodedPacket, PcmFormat, Result, StreamSpec};
use std::sync::{Arc, Mutex, OnceLock};
pub const ALL_TRACK_KBPS: i32 = 192;
pub const SOURCE_TRACK_KBPS: i32 = 160;
pub struct TrackPacket {
    pub track_index: usize,
    pub packet: EncodedPacket,
}
#[derive(Clone, Debug)]
pub struct AudioTrackSpec {
    pub name: &'static str,
    pub stream: Option<StreamSpec>,
}
struct Track {
    name: &'static str,
    encoder: Option<AacAudioEncoder>,
}
pub struct AudioTracks {
    tracks: Mutex<Vec<Track>>,
    separate: bool,
}
impl AudioTracks {
    pub fn new(system_format: PcmFormat, separate: bool) -> Result<Self> {
        let tracks = if separate {
            vec![
                Track { name: "All audio", encoder: Some(AacAudioEncoder::new(system_format, ALL_TRACK_KBPS)?) },
                Track { name: "System audio", encoder: Some(AacAudioEncoder::new(system_format, SOURCE_TRACK_KBPS)?) },
                Track { name: "Microphone", encoder: None },
            ]
        } else {
            vec![Track { name: "Audio", encoder: Some(AacAudioEncoder::new(system_format, SOURCE_TRACK_KBPS)?) }]
        };
        Ok(Self { tracks: Mutex::new(tracks), separate })
    }
    pub fn specs(&self) -> Vec<AudioTrackSpec> {
        self.tracks
            .lock()
            .unwrap()
            .iter()
            .map(|t| AudioTrackSpec { name: t.name, stream: t.encoder.as_ref().map(AacAudioEncoder::get_stream_spec) })
            .collect()
    }
    pub fn encode_system(&self, bytes: &[u8], engine_us: i64) -> Result<Vec<TrackPacket>> {
        let tracks = self.tracks.lock().unwrap();
        let mut out = Vec::new();
        for (i, t) in tracks.iter().take(if self.separate { 2 } else { 1 }).enumerate() {
            append(&mut out, i, t.encoder.as_ref().unwrap().encode(bytes, engine_us)?);
        }
        Ok(out)
    }
    pub fn encode_mic(&self, bytes: &[u8], format: PcmFormat, engine_us: i64) -> Result<Vec<TrackPacket>> {
        let mut tracks = self.tracks.lock().unwrap();
        let mut out = Vec::new();
        tracks[0].encoder.as_ref().unwrap().encode_mic(bytes, format)?;
        if self.separate && !bytes.is_empty() {
            if tracks[2].encoder.is_none() {
                tracks[2].encoder = Some(AacAudioEncoder::new(format, SOURCE_TRACK_KBPS)?);
            }
            append(&mut out, 2, tracks[2].encoder.as_ref().unwrap().encode(bytes, engine_us)?);
        }
        Ok(out)
    }
    pub fn pump_silence_to(&self, engine_us: i64) -> Result<Vec<TrackPacket>> {
        let tracks = self.tracks.lock().unwrap();
        let mut out = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            if let Some(e) = &t.encoder {
                append(&mut out, i, e.pump_silence_to(engine_us)?);
            }
        }
        Ok(out)
    }
    pub fn flush(&self) -> Result<Vec<TrackPacket>> {
        let tracks = self.tracks.lock().unwrap();
        let mut out = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            if let Some(e) = &t.encoder {
                append(&mut out, i, e.flush()?);
            }
        }
        Ok(out)
    }
}
fn append(out: &mut Vec<TrackPacket>, index: usize, packets: Vec<EncodedPacket>) {
    out.extend(packets.into_iter().map(|packet| TrackPacket { track_index: index, packet }));
}
type Sink = Arc<dyn Fn(TrackPacket) + Send + Sync>;
/// Optional engine wiring: raw source, shared QPC clock, lazy mic and 100ms silence timer.
pub struct AudioSession {
    source: crate::WasapiAudioSource,
    tracks: Arc<OnceLock<AudioTracks>>,
    clock: Arc<crate::clock::EngineClock>,
    sink: Sink,
    stop: crossbeam_channel::Sender<()>,
    pump: Option<std::thread::JoinHandle<()>>,
    // Serialize encode + publication, so concurrent pump/mic/system callers cannot reorder packets.
    publish: Arc<Mutex<()>>,
}
impl AudioSession {
    pub fn start(
        options: crate::AudioSourceOptions,
        controls: Arc<crate::AudioControls>,
        separate: bool,
        clock: Arc<crate::clock::EngineClock>,
        sink: impl Fn(TrackPacket) + Send + Sync + 'static,
    ) -> Result<Self> {
        let tracks = Arc::new(OnceLock::<AudioTracks>::new());
        let sink: Sink = Arc::new(sink);
        let publish = Arc::new(Mutex::new(()));
        let t = tracks.clone();
        let c = clock.clone();
        let s = sink.clone();
        let order = publish.clone();
        let system = Arc::new(move |b: &[u8], _: PcmFormat| {
            let _order = order.lock().unwrap();
            if c.is_paused() {
                return;
            }
            if let Some(t) = t.get() {
                dispatch(t.encode_system(b, c.engine_now_us()), &s);
            }
        });
        let t = tracks.clone();
        let c = clock.clone();
        let s = sink.clone();
        let order = publish.clone();
        let mic = Arc::new(move |b: &[u8], f: PcmFormat| {
            let _order = order.lock().unwrap();
            if c.is_paused() {
                return;
            }
            if let Some(t) = t.get() {
                dispatch(t.encode_mic(b, f, c.engine_now_us()), &s);
            }
        });
        let source = crate::WasapiAudioSource::start_with_controls(options, controls, system, mic)?;
        let _ = tracks.set(AudioTracks::new(source.game_format(), separate)?);
        let (stop, rx) = crossbeam_channel::bounded(1);
        let t = tracks.clone();
        let c = clock.clone();
        let s = sink.clone();
        let order = publish.clone();
        let pump = std::thread::Builder::new().name("AudioSilencePump".into()).spawn(move || {
            let timer = crossbeam_channel::tick(std::time::Duration::from_millis(100));
            loop {
                crossbeam_channel::select! { recv(rx) -> _ => break, recv(timer) -> _ => {
                    let _order = order.lock().unwrap();
                    dispatch(t.get().unwrap().pump_silence_to(c.engine_now_us()), &s);
                } }
            }
        })?;
        Ok(Self { source, tracks, clock, sink, stop, pump: Some(pump), publish })
    }
    pub fn tracks(&self) -> &AudioTracks {
        self.tracks.get().unwrap()
    }
    pub fn controls(&self) -> Arc<crate::AudioControls> {
        self.source.controls()
    }
    /// Call immediately before a save so the audio tail reaches the current engine time.
    pub fn pump(&self) {
        let _order = self.publish.lock().unwrap();
        dispatch(self.tracks().pump_silence_to(self.clock.engine_now_us()), &self.sink);
    }
    /// Stops callbacks before flushing. Drop only stops; explicit finish publishes delayed AAC packets.
    pub fn finish(&mut self) -> Result<()> {
        self.stop_workers();
        let _order = self.publish.lock().unwrap();
        for p in self.tracks().flush()? {
            (self.sink)(p);
        }
        Ok(())
    }
    fn stop_workers(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(t) = self.pump.take() {
            let _ = t.join();
        }
        self.source.stop();
    }
}
impl Drop for AudioSession {
    fn drop(&mut self) {
        self.stop_workers();
    }
}
fn dispatch(result: Result<Vec<TrackPacket>>, sink: &Sink) {
    match result {
        Ok(packets) => {
            for p in packets {
                sink(p);
            }
        }
        Err(e) => log::warn!("[VfrReplayEngine] audio pump error: {e}"),
    }
}

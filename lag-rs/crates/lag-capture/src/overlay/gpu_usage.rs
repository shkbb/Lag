//! Port of `Services/VfrCapture/GpuUsageProbe.cs` and PDH sampling in `SystemStatsSampler.cs`.
use std::collections::HashMap;
use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::*;

#[derive(Default)]
pub struct GpuUsageProbe {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    opened: bool,
    last: HashMap<u32, f64>,
}
pub(crate) struct GpuSample {
    by_pid: HashMap<u32, f64>,
    pub total: f64,
}
impl GpuUsageProbe {
    pub fn new() -> Self {
        Self::default()
    }
    fn ensure_open(&mut self) {
        if self.opened {
            return;
        }
        self.opened = true;
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut self.query) != 0 {
                return;
            }
            if PdhAddEnglishCounterW(
                self.query,
                w!(r"\GPU Engine(*engtype_3D)\Utilization Percentage"),
                0,
                &mut self.counter,
            ) != 0
            {
                self.counter = PDH_HCOUNTER::default();
                return;
            }
            PdhCollectQueryData(self.query);
        }
    }
    /// Failed reads preserve the previous detector sample, just as in C#.
    pub fn sample(&mut self) {
        if let Some(mut sample) = self.collect() {
            sample.by_pid.remove(&0);
            self.last = sample.by_pid;
        }
    }
    pub fn get_3d(&self, pid: u32) -> f64 {
        self.last.get(&pid).copied().unwrap_or(0.0)
    }
    pub(crate) fn collect(&mut self) -> Option<GpuSample> {
        self.ensure_open();
        if self.counter.0.is_null() {
            return None;
        }
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut bytes, mut count) = (0, 0);
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) != 0x800007d2
                || bytes == 0
            {
                return None;
            }
            // Native PDH items contain an eight-byte aligned double union.
            let mut storage = vec![0u64; (bytes as usize).div_ceil(8)];
            let ptr = storage.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(ptr)) != 0 {
                return None;
            }
            if count as usize > storage.len() * 8 / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() {
                return None;
            }
            let mut map = HashMap::new();
            let mut total = 0.0;
            for item in std::slice::from_raw_parts(ptr, count as usize) {
                if item.FmtValue.CStatus != 0 {
                    continue;
                }
                total += item.FmtValue.Anonymous.doubleValue;
                // Names point into this allocation; bound UTF-16 reads to its end.
                let start = item.szName.0 as usize;
                let base = storage.as_ptr() as usize;
                let end = base + storage.len() * 8;
                if start < base || start >= end || !start.is_multiple_of(2) {
                    continue;
                }
                let wide = std::slice::from_raw_parts(item.szName.0, (end - start) / 2);
                let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
                let pid = parse_pid(&String::from_utf16_lossy(&wide[..len]));
                // Stats sum all valid instances, including those without a usable PID.
                *map.entry(pid).or_insert(0.0) += item.FmtValue.Anonymous.doubleValue;
            }
            Some(GpuSample { by_pid: map, total })
        }
    }
}
pub(crate) fn parse_pid(instance: &str) -> u32 {
    let Some(i) = instance.find("pid_") else {
        return 0;
    };
    instance[i + 4..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u32, |n, c| n.wrapping_mul(10).wrapping_add((c - b'0') as u32))
}
impl Drop for GpuUsageProbe {
    fn drop(&mut self) {
        if !self.query.0.is_null() {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_layout_and_pid() {
        assert_eq!(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>(), 24);
        assert_eq!(parse_pid("pid_1234_luid_0x_engtype_3D"), 1234);
        assert_eq!(parse_pid("prefix_pid_42_x"), 42);
        assert_eq!(parse_pid("pid_no"), 0);
        assert_eq!(parse_pid("none"), 0);
    }
}

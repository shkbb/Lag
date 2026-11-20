//! Cloud storage for replays: the model every provider and the UI share. The design and its rules are in
//! `docs/spec/cloud.md`; each module below implements one section of it.
//! * `ids`      clip ids (stable across PCs), account ids, provider kinds;
//! * `layout`   the folder tree in the cloud (root / game / month) and names safe on every provider;
//! * `index`    the cloud index (`.lag/library.json`): entries, tombstones, merge between PCs;
//! * `presence` where a clip lives (here, in which clouds, in flight) and the library = local ∪ clouds;
//! * `settings` the user's cloud settings and which accounts a clip goes to;
//! * `queue`    the persistent transfer queue: dedupe, priorities, retry with backoff, auth pauses;
//! * `quota`    the library size limit and the cloud's auto-cleanup, with the cloud in mind;
//! * `provider` what every cloud implements (Google Drive, Dropbox, OneDrive...), errors, streaming;
//! * `sync`     one account: index read-merge-write, upload with thumbnail, delete, cleanup, rescan;
//! * `memory`   an in-memory cloud with injectable faults (tests, offline previews).
pub mod ids;
pub mod index;
pub mod layout;
pub mod memory;
pub mod presence;
pub mod provider;
pub mod queue;
pub mod quota;
pub mod settings;
pub mod sync;

pub use ids::{AccountId, ClipId, ProviderKind};

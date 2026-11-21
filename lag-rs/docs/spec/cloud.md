# Cloud storage (design, 2026-11-20)

Owner's brief: replays go to the user's own cloud (Google Drive first, others alike), into Lag's own folder
tree sorted by game; the library shows every clip with where it lives (here / in the cloud / both); a new PC
that connects the same cloud sees the whole library again; several clouds can be combined; many settings.
"Everything has to be thought through": this file is the contract the code follows.

Code: `crates/lag-cloud` (pure Rust, no Win32, no network in the model). Network providers and the UI come on
top of it. Every rule below has a test there.

## 1. Words
| term | meaning |
|---|---|
| **account** | one connected cloud: provider (Google Drive, Dropbox, OneDrive, ...) + the signed-in user |
| **clip id** | stable id of a clip, independent of its path (`ClipId`, a UUID). Stored in the clip's metadata the first time the clip meets the cloud; the same clip keeps it on every PC |
| **cloud index** | `library.json` in the account's system folder: every clip the account holds, with its metadata. What a new PC reads to rebuild the library |
| **presence** | where a clip is: local file? which accounts have it? anything in flight? |
| **job** | one unit of transfer work in the persistent queue (upload, download, delete, metadata sync) |

## 2. Folder tree in the cloud
```
<root>/                      root name: setting, default "Lag"
  Counter-Strike 2/          one folder per game (setting: sort mode, below)
    2026-11/                 optional month level
      2026-11-20_00-19-35-Replay.mp4
  Desktop/                   clips with no game
  .lag/                      system folder, never sorted, never shown as a game
    library.json             the cloud index (§4)
    thumbs/<clip id>.jpg     thumbnails, so a new PC shows the grid before downloading anything
```
Sort modes: `ByGame` (default), `ByGameMonth`, `ByMonth`, `Flat`. Changing the mode applies to new uploads;
"reorganise" moves existing files (a separate, explicit action).

Names are made safe for every provider at once (one rule set, the strictest): no `< > : " / \ | ? *`, no
control chars, no trailing dots/spaces, no reserved Windows names (`CON`, `NUL`, `COM1`...), at most 120 chars,
never empty (falls back to `Desktop`/`Unknown`). Game folder names come from the same game name the library
chips show.

**The path is for humans; the app never depends on it.** A clip is found by its clip id: the cloud index has
the provider's file id, and the file carries the clip id as provider metadata (Drive `appProperties`,
Dropbox/OneDrive equivalents). The user may rename or move files inside the cloud; Lag still finds them
(full rescan of `<root>` matches by that metadata, then by name + size).

## 3. Clip identity across PCs
- Local metadata (`ClipMetadata`) gains `CloudId` and per-account remote refs. Written the first time a clip
  is queued for upload.
- A clip that arrives from the cloud (new PC) is written to the library folder and gets the SAME id in its
  local metadata, so it is "both" immediately, never a duplicate.
- Matching a local file without an id against the index (reinstall, metadata cache wiped): by file name +
  size + duration (all three), then it adopts the index's id.
- Content hash: SHA-256 is stored after upload when the provider reports one (Drive md5/sha256, Dropbox
  content_hash); used to detect a changed file (an edited clip overwritten in place = a new revision).

## 4. Cloud index (`.lag/library.json`)
Versioned (`"schema": 1`). One entry per clip id: file name, remote file id, remote folder path, game, exe,
created (UTC), duration, size, hash, favorite, edited, thumbnail ref, uploaded-at, uploaded-by (device name),
`updated` (UTC, per entry) and `deleted` (tombstone time).

Several PCs write the same index, so updates are **merge, never overwrite**:
1. read index + its revision (etag),
2. merge the local changes entry by entry: the newer `updated` wins; a tombstone beats an older entry and
   is itself replaced by a newer entry (clip re-uploaded),
3. write with "only if revision is still X"; on conflict read again and redo 2-3.
Tombstones are kept 90 days so an offline PC does not resurrect a deleted clip.

The index is a cache: if it is missing or corrupt, it is rebuilt from a rescan of `<root>` (file metadata
carries the clip id, game, created, duration).

## 5. What the library shows
The library = local scan ∪ every enabled account's index, keyed by clip id. Each card gets a presence:

| presence | meaning | card |
|---|---|---|
| `Local` | only on this PC | no badge |
| `Queued` / `Uploading(p)` | going up | progress ring |
| `Synced` | here and in the cloud | cloud-check badge |
| `CloudOnly` | in the cloud, not on this PC | cloud badge, thumbnail from `.lag/thumbs`; play = download (or stream later) |
| `Downloading(p)` | coming down | progress ring |
| `Failed(reason)` | last transfer failed | warning badge, retry in the menu |

With several accounts the badge shows the best state and the tooltip lists each account.

## 6. Several clouds together
Each account has a **role**:
- `Primary`: gets every clip the upload rule selects; its index is what the library shows first.
- `Mirror`: a second copy (backup) of the same selection, or only favorites (setting per mirror).
- `Paused`: connected, shown in the library, but nothing new goes up.
One primary at most; any number of mirrors. A clip is uploaded per account independently (one slow or full
cloud never blocks another).

## 7. Settings
Global: enabled; root folder name; sort mode; what to upload (`All` new replays / `Favorites` / `Manual`
only); when (`Immediately` / `When no game is running` / `Only while idle`); upload speed limit; upload
screenshots too; keep local copies (`Always` / free local space for clips older than N days once uploaded);
delete behaviour (`This PC only` / `Everywhere`, asked each time by default).
Per account: role, mirror selection, connected user (shown), sign out.

Recording comes first: uploads pause while a game is in the foreground unless the user says otherwise, and
run at below-normal I/O priority.

## 8. Queue
Persistent (`%LOCALAPPDATA%\Lag\cloud\queue.json`), survives restarts and crashes. Jobs: `Upload`,
`Download`, `Delete`, `SyncMeta` (favorite/edited/game changed), each for (clip, account). Rules:
- one job per (clip, account, kind); a newer one replaces a queued older one (favorite toggled twice = one
  sync), a delete cancels a queued upload of the same clip;
- uploads are resumable: the provider's upload session (URL + offset) is stored in the job, so a restart or a
  dropped connection continues where it stopped;
- failures retry with backoff 30 s, 2 min, 10 min, 1 h, then stay `Failed` until the user retries; auth
  failures stop that account's jobs until it is signed in again;
- a clip still being written (the replay is being saved, the editor exports) is not uploaded until the file
  is closed and stable.

## 9. Storages, sign-in and secrets
| storage | crate module | sign-in | where Lag writes | file properties |
|---|---|---|---|---|
| Google Drive | `drive` | OAuth PKCE, loopback `127.0.0.1:<any port>`, client id + secret from `cloud-secrets.env` | `Lag/` in My Drive, scope `drive.file` (only Lag's files) | Drive `appProperties` |
| Dropbox | `dropbox` | OAuth PKCE (no secret), loopback `http://localhost:53982` (registered exactly) | the app folder `Apps/<app>/Lag/` | notes in `Lag/.lag/props` |
| OneDrive (personal) | `onedrive` | OAuth PKCE (no secret), loopback `http://localhost:<any port>` | the app folder `Apps/<app>/Lag/` (scope `Files.ReadWrite.AppFolder`) | notes in `.lag-props` (by item id) |
| Network folder / NAS | `folder` | none: a folder Lag can write to (`\\nas\share`, mapped drive, synced folder) | `<folder>/Lag/` | notes in `Lag/.lag/props` |
| WebDAV (Nextcloud, ownCloud, Synology, QNAP, TrueNAS, Seafile...) | `webdav` | login + app password, checked by PROPFIND before it is added | `<webdav folder>/Lag/` | notes in `Lag/.lag/props` |

OAuth: the system browser opens, the user signs in, the app receives the code on the loopback (`oauth::Service`
describes each provider). Tokens and WebDAV passwords are stored per account encrypted with Windows DPAPI (user
scope, `%LOCALAPPDATA%\Lag\cloud\*.token|*.secret`), never in settings. OAuth client ids come from the gitignored
`lag-rs/cloud-secrets.env` at build time (`GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `DROPBOX_APP_KEY`,
`ONEDRIVE_CLIENT_ID`); a provider without its key is simply not offered. Desktop apps cannot keep a secret; PKCE is
what makes the flow safe. Index writes are conditional everywhere: Drive `version`, Dropbox `rev` +
`strict_conflict`, OneDrive / WebDAV `If-Match` ETag, NAS a lock file + modified time.

## 11. Every library feature with the cloud
Owner, 2026-11-20: "all library functions have to work, the storage limit and so on". Each one, explicitly:

| feature | with the cloud |
|---|---|
| **Library size limit** (auto-cleanup, oldest first, screenshots not counted) | Same limit, local files only (cloud-only clips take no disk). First removes the LOCAL COPIES of the oldest clips already in a cloud (lossless: they stay in the library as cloud-only); only then deletes the oldest local-only clips, as today. Never a clip in transfer. (`quota::plan_local_limit`) |
| **Cloud auto-cleanup** (new, per account; owner: "the cloud needs a limit too") | Exactly like the local one: a switch (OFF by default) + a GB limit for Lag's clips in that cloud. On: before each upload the oldest clips (never favorites) are removed from that cloud until the new one fits the limit; if it still cannot fit, the upload fails with "over the cloud limit". A cloud that is full for any other reason (the user's own files) NEVER makes Lag delete anything (owner, 2026-11-20): the upload fails with "not enough space in <cloud>". (`quota::plan_cloud_limit`, `sync::prepare_upload`) |
| **Free local space after N days** (new, off by default) | Local copies older than N days that are in a cloud are removed; they stay playable from the cloud. (`quota::plan_free_space`) |
| **Disk usage bar** | Unchanged for the disk. Settings > Cloud shows per account: Lag's clips (count, GB), free / total in that cloud, the auto-cleanup switch and limit. (`sync::usage`) |
| **Favorites** (star, favorites-only filter) | Toggling writes the index (`SyncMeta`), so other PCs see it; with "upload favorites" it also queues the upload; favorite mirrors receive it. Filter works on cloud-only clips too. |
| **Game filter chips, search, date groups** | Run on the joined library: cloud-only clips have game, file name and date from the index. Chips count both. |
| **Play** | Local: as today. Cloud-only: plays straight from the cloud (stream, seeking works); "Save to this PC" downloads it into the library folder (owner, 2026-11-20). (`sync::stream`) |
| **Edit / montage** | Needs local files: missing ones are downloaded first. The export is a NEW clip (new id) and follows the upload rule. |
| **Delete** (one, selected, context menu) | See §12. |
| **Show in folder** | Local: as today. Cloud-only: "Open in <cloud>" (web link to the file). |
| **Open library folder** | Unchanged. |
| **Thumbnails** | Local: as today. Cloud-only: `.lag/thumbs/<id>.jpg` downloaded into the thumbnail cache. Uploaded together with the clip. |
| **Clip duration / metadata cache** | Stored in the index too, so a new PC shows durations without probing files. |
| **Library folder changed** | Cloud entries stay; local presence is rescanned; nothing is re-uploaded (ids in metadata). |
| **Library refresh** | Refreshes local scan + every account index (index read is cheap; full rescan of the cloud only on demand). |

## 10. Order of work
1. DONE: `lag-cloud` model: ids, layout + names, index + merge, presence, settings + policy, queue, quota.
2. DONE: `CloudProvider` trait, in-memory provider with faults, `sync::AccountClient`; end-to-end tests
   (tests/two_pcs.rs): PC B sees PC A's library, concurrent index writes, resumed upload, full cloud cleanup,
   corrupt index rebuilt from the files, expired sign-in.
3. DONE in code (crate lag-cloud-drive, 19 tests on scripted HTTP): Google Drive provider (REST v3, resumable
   upload that asks Google where it stands, appProperties), OAuth PKCE + loopback, DPAPI token store.
   Live check: `cargo run -p lag-cloud-drive --example drive_live --release` (opens the browser).
   PASSED on the owner's Drive 2026-11-20: sign-in, 20 MB in 3 chunks (9.9 s), byte-exact download, stream URL,
   rescan without the index, delete + tombstone.
4. DONE: queue worker (`lag-app/src/services/cloud/worker.rs`): uploads after save by the rules, pause while a
   game runs (resumes the session), index copies on disk, thumbnails of cloud clips.
5. DONE: Settings > Library (storages, roles, rules, delete mode), library badges + cloud-only cards + menus,
   play from the cloud, save to this PC, delete flow (§12), size limit evicting cloud copies first.
6. DONE in code: Network folder / NAS, WebDAV, Dropbox, OneDrive providers (same trait; 36 provider tests on
   scripted HTTP and real folders). Live checks (`examples/drive_live.rs`, `examples/oauth_live.rs`): Google Drive
   and Dropbox passed (sign-in, refresh, 20 MB in chunks, byte-exact download, stream link, rescan by notes,
   delete + tombstone). A first Dropbox sign-in can answer "busy" while it makes the app folder: sign-in calls
   retry 429 / 5xx with pauses. NAS / WebDAV need the owner's storages; OneDrive needs an app registration, which
   Microsoft no longer allows from a personal account without an Azure directory (deferred; a synced OneDrive
   folder works as a Network folder meanwhile).
7. NOT YET: speed limit; a speed / bandwidth setting is not in the UI.

## Decisions (owner, 2026-11-20)
- Cloud-only clips play from the cloud; the user can save them locally.
- Screenshots are uploaded too (default on), into the same game folders.
- Free local space after N days: off by default.
- Cloud auto-cleanup per account: OFF by default, only the user's own limit, never favorites; a full cloud is an
  upload error, never a reason to delete.

## 12. Delete (approved by the owner, 2026-11-20)
What "Delete" offers depends on where the clip is, so the user never has to know the setting:
- **Only on this PC**: as today (confirm, delete).
- **Here and in a cloud**: the confirmation offers three buttons: "Delete from this PC" (stays in the cloud,
  becomes cloud-only, frees disk), "Delete from the cloud" (stays here), "Delete everywhere". A "Remember my
  choice" box stores it (`DeleteMode`); Settings > Cloud can reset it.
- **Only in the cloud**: "Delete from the cloud" with a warning that it is gone for good.
- **Several selected**: one confirmation; each clip gets what it can (a local-only clip ignores "from the cloud").
- Deleting "everywhere" or "from the cloud" removes the clip from every connected cloud that has it, and other
  PCs drop it (tombstone).

## Open questions (owner)
- none right now

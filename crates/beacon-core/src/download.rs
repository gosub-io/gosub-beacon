//! In-flight and finished downloads for this session.
//!
//! The engine does the transferring and reports progress by its own download id; this is
//! the shell's side of it — what the user sees in the downloads list, and the bookkeeping
//! that keeps it in step.

use std::path::PathBuf;

/// One download, as shown in the downloads list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadEntry {
    pub id: u64,
    pub filename: String,
    pub path: PathBuf,
    pub received: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    Running,
    Finished,
    Failed(String),
}

impl DownloadEntry {
    /// Fraction complete, or `None` when the server never said how big it is.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(total) if total > 0 => Some((self.received as f64 / total as f64).clamp(0.0, 1.0)),
            _ => None,
        }
    }
}

/// This session's downloads, oldest first, plus the id source.
#[derive(Debug, Default, Clone)]
pub struct Downloads {
    entries: Vec<DownloadEntry>,
    next_id: u64,
}

impl Downloads {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            // Ids start at 1 so 0 never reads as a real download.
            next_id: 1,
        }
    }

    pub fn entries(&self) -> &[DownloadEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Claim the next download id, before the engine has been asked to start it.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Record a download that has just been started.
    pub fn start(&mut self, id: u64, filename: String, path: PathBuf) {
        self.entries.push(DownloadEntry {
            id,
            filename,
            path,
            received: 0,
            total: None,
            state: DownloadState::Running,
        });
    }

    /// Apply an update to one entry. Returns whether the entry existed — progress for an
    /// unknown id means the engine outlived our record of it, which is worth ignoring
    /// rather than panicking over.
    pub fn update(&mut self, id: u64, apply: impl FnOnce(&mut DownloadEntry)) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(entry) => {
                apply(entry);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, id: u64) -> Option<&DownloadEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downloads_with_one() -> (Downloads, u64) {
        let mut d = Downloads::new();
        let id = d.next_id();
        d.start(id, "file.zip".into(), PathBuf::from("/tmp/file.zip"));
        (d, id)
    }

    #[test]
    fn ids_are_unique_and_never_zero() {
        let mut d = Downloads::new();
        let a = d.next_id();
        let b = d.next_id();
        assert_ne!(a, b);
        assert!(a > 0);
    }

    #[test]
    fn progress_updates_the_named_entry() {
        let (mut d, id) = downloads_with_one();
        assert!(d.update(id, |e| {
            e.received = 512;
            e.total = Some(1024);
        }));
        let entry = d.get(id).unwrap();
        assert_eq!(entry.received, 512);
        assert_eq!(entry.fraction(), Some(0.5));
    }

    #[test]
    fn progress_for_an_unknown_id_is_ignored_not_fatal() {
        let (mut d, id) = downloads_with_one();
        assert!(!d.update(id + 999, |e| e.received = 1));
        assert_eq!(d.get(id).unwrap().received, 0);
    }

    #[test]
    fn a_download_of_unknown_size_has_no_fraction() {
        let (d, id) = downloads_with_one();
        assert_eq!(d.get(id).unwrap().fraction(), None);
    }

    #[test]
    fn a_zero_length_download_has_no_fraction_rather_than_dividing_by_zero() {
        let (mut d, id) = downloads_with_one();
        d.update(id, |e| e.total = Some(0));
        assert_eq!(d.get(id).unwrap().fraction(), None);
    }

    #[test]
    fn overshooting_the_reported_total_clamps_at_one() {
        let (mut d, id) = downloads_with_one();
        d.update(id, |e| {
            e.total = Some(100);
            e.received = 150;
        });
        assert_eq!(d.get(id).unwrap().fraction(), Some(1.0));
    }

    #[test]
    fn failure_is_recorded_with_its_reason() {
        let (mut d, id) = downloads_with_one();
        d.update(id, |e| e.state = DownloadState::Failed("connection reset".into()));
        assert_eq!(d.get(id).unwrap().state, DownloadState::Failed("connection reset".into()));
    }
}

//! One local operation at a time; dialogs and credential/file I/O never block egui or BLE.
use crate::profiles::{self, Catalog, Manager};
use ble_session::provisioning::Profile;
use std::{future::Future, path::PathBuf, pin::Pin, sync::mpsc, thread};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestinationContext {
    epoch: u64,
    connection: companion_core::Connection,
}
impl DestinationContext {
    pub fn from_snapshot(snapshot: &companion_core::Snapshot) -> Self {
        Self {
            epoch: snapshot.epoch,
            connection: snapshot.connection,
        }
    }
}

type Dialog = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>>;
enum Request {
    Load(bool),
    Import(Dialog),
    Select(String),
    Destination {
        dialog: Dialog,
        context: DestinationContext,
        draft: String,
    },
}
pub enum Reply {
    Profiles {
        catalog: Catalog,
        profile: Option<Profile>,
    },
    Destination {
        path: PathBuf,
        context: DestinationContext,
        draft: String,
    },
    Cancelled,
    Error(&'static str),
    DestinationError(&'static str),
}
pub struct Desktop {
    requests: Option<mpsc::SyncSender<Request>>,
    results: mpsc::Receiver<Reply>,
    worker: Option<thread::JoinHandle<()>>,
    pub busy: bool,
}
impl Desktop {
    pub fn new(repaint: eframe::egui::Context, load: bool, restore: bool) -> std::io::Result<Self> {
        let (requests, rx) = mpsc::sync_channel(1);
        let (tx, results) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("companion-desktop".into())
            .spawn(move || {
                let mut manager = Manager::open();
                while let Ok(request) = rx.recv() {
                    let reply = match request {
                        Request::Destination {
                            dialog,
                            context,
                            draft,
                        } => match futures_executor::block_on(dialog) {
                            None => Reply::Cancelled,
                            Some(file) => match validate_destination(file.path()) {
                                Ok(()) => Reply::Destination {
                                    path: file.path().to_owned(),
                                    context,
                                    draft,
                                },
                                Err(error) => Reply::DestinationError(error),
                            },
                        },
                        Request::Import(dialog) => match futures_executor::block_on(dialog) {
                            None => Reply::Cancelled,
                            Some(file) => match &mut manager {
                                Ok(manager) => profile_reply(
                                    manager.import(file.path()).map(|(c, p)| (c, Some(p))),
                                ),
                                Err(error) => Reply::Error(error.message()),
                            },
                        },
                        Request::Select(id) => match &mut manager {
                            Ok(manager) => {
                                profile_reply(manager.select(&id).map(|(c, p)| (c, Some(p))))
                            }
                            Err(error) => Reply::Error(error.message()),
                        },
                        Request::Load(restore) => {
                            // Retry also retries opening a previously unavailable home/config directory.
                            if manager.is_err() {
                                manager = Manager::open();
                            }
                            match &manager {
                                Ok(manager) => profile_reply(manager.load(restore)),
                                Err(error) => Reply::Error(error.message()),
                            }
                        }
                    };
                    if tx.send(reply).is_err() {
                        break;
                    }
                    repaint.request_repaint();
                }
            })?;
        let mut desktop = Self {
            requests: Some(requests),
            results,
            worker: Some(worker),
            busy: false,
        };
        if load {
            let _ = desktop.submit(Request::Load(restore));
        }
        Ok(desktop)
    }
    fn submit(&mut self, request: Request) -> Result<(), &'static str> {
        if self.busy {
            return Err("Finish the current desktop operation first");
        }
        self.requests
            .as_ref()
            .ok_or("Desktop worker stopped")?
            .try_send(request)
            .map_err(|_| "Desktop operation unavailable")?;
        self.busy = true;
        Ok(())
    }
    pub fn load(&mut self) -> Result<(), &'static str> {
        self.submit(Request::Load(true))
    }
    pub fn select(&mut self, id: String) -> Result<(), &'static str> {
        self.submit(Request::Select(id))
    }
    pub fn import(&mut self) -> Result<(), &'static str> {
        if self.busy {
            return Err("Finish the current desktop operation first");
        }
        // Start on the window/main thread, await on our single worker, as required by AppKit.
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Import private Pico provisioning profile")
            .add_filter("Pico profile", &["json"])
            .pick_file();
        self.submit(Request::Import(Box::pin(dialog)))
    }
    pub fn destination(
        &mut self,
        context: DestinationContext,
        draft: String,
    ) -> Result<(), &'static str> {
        if self.busy {
            return Err("Finish the current desktop operation first");
        }
        let mut picker = rfd::AsyncFileDialog::new().set_title("Choose a new download filename");
        let current = std::path::Path::new(&draft);
        if current.is_absolute() {
            if let Some(parent) = current.parent() {
                picker = picker.set_directory(parent);
            }
            if let Some(name) = current.file_name().and_then(|s| s.to_str()) {
                picker = picker.set_file_name(name);
            }
        }
        let dialog = picker.save_file();
        self.submit(Request::Destination {
            dialog: Box::pin(dialog),
            context,
            draft,
        })
    }
    pub fn poll(&mut self) -> Option<Reply> {
        match self.results.try_recv() {
            Ok(reply) => {
                self.busy = false;
                Some(reply)
            }
            Err(mpsc::TryRecvError::Disconnected) if self.busy => {
                self.busy = false;
                Some(Reply::Error("Desktop worker stopped; restart the app"))
            }
            _ => None,
        }
    }
}
fn profile_reply(result: Result<(Catalog, Option<Profile>), profiles::Error>) -> Reply {
    match result {
        Ok((catalog, profile)) => Reply::Profiles { catalog, profile },
        Err(error) => Reply::Error(error.message()),
    }
}
fn validate_destination(path: &std::path::Path) -> Result<(), &'static str> {
    if path.to_str().is_none()
        || !(companion_core::download::Command::QueueDefault {
            destination: path.to_owned(),
        })
        .valid()
    {
        return Err("Choose a new absolute local filename with valid UTF-8");
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err("Destination exists; choose a new filename. Nothing was overwritten"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Cannot inspect destination; choose an accessible directory"),
    }
}
pub fn apply_destination(
    context: DestinationContext,
    draft: &str,
    path: &std::path::Path,
    current_context: DestinationContext,
    current: &mut String,
) -> Result<(), &'static str> {
    if context != current_context || draft != current {
        return Err("Connection or destination draft changed; choose the filename again");
    }
    *current = path
        .to_str()
        .ok_or("Choose a filename with valid UTF-8")?
        .into();
    Ok(())
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.requests.take();
        // Native dialogs/credential prompts cannot reliably be interrupted. They
        // must never prevent scoped BLE/HID cleanup or keep the app open on exit.
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_native_dialog_cannot_block_application_shutdown() {
        let mut desktop = Desktop::new(eframe::egui::Context::default(), false, false).unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        desktop
            .submit(Request::Destination {
                dialog: Box::pin(async move { wait.await.unwrap_or(None) }),
                context: DestinationContext {
                    epoch: 4,
                    connection: companion_core::Connection::Connected,
                },
                draft: String::new(),
            })
            .unwrap();
        let start = std::time::Instant::now();
        drop(desktop);
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
        let _ = release.send(None);
    }
    #[test]
    fn late_save_dialog_result_cannot_replace_a_new_draft_or_connection() {
        let connected = DestinationContext {
            epoch: 4,
            connection: companion_core::Connection::Connected,
        };
        let older = DestinationContext {
            epoch: 3,
            ..connected
        };
        let disconnected = DestinationContext {
            connection: companion_core::Connection::Disconnected,
            ..connected
        };
        let mut current = "/new-draft.bin".to_string();
        assert!(
            apply_destination(
                connected,
                "/old-draft.bin",
                "/chosen.bin".as_ref(),
                connected,
                &mut current
            )
            .is_err()
        );
        assert_eq!(current, "/new-draft.bin");
        assert!(
            apply_destination(
                older,
                "/new-draft.bin",
                "/chosen.bin".as_ref(),
                connected,
                &mut current
            )
            .is_err()
        );
        assert_eq!(current, "/new-draft.bin");
        assert!(
            apply_destination(
                connected,
                "/new-draft.bin",
                "/chosen.bin".as_ref(),
                disconnected,
                &mut current
            )
            .is_err()
        );
        assert_eq!(current, "/new-draft.bin");
        apply_destination(
            connected,
            "/new-draft.bin",
            "/chosen.bin".as_ref(),
            connected,
            &mut current,
        )
        .unwrap();
        assert_eq!(current, "/chosen.bin");
    }
    #[test]
    fn save_selection_never_overwrites_and_rejects_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing.bin");
        std::fs::write(&path, b"existing").unwrap();
        assert!(validate_destination(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        assert!(validate_destination(&directory.path().join("new.bin")).is_ok());
        assert!(validate_destination(std::path::Path::new("relative.bin")).is_err());
        #[cfg(unix)]
        {
            let link = directory.path().join("dangling");
            std::os::unix::fs::symlink("missing", &link).unwrap();
            assert!(validate_destination(&link).is_err());
        }
    }
}

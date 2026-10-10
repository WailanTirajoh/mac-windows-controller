//! Daftar transfer file untuk UI, dan perintah untuk mengirim serta mengelolanya.

use std::path::{Path, PathBuf};

use sambung_core::node::TransferEvent;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::commands::ensure_peer;
use crate::state::{AppState, EVENT_TRANSFERS_CHANGED};

type CommandResult<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    id: u64,
    name: String,
    size: u64,
    transferred: u64,
    /// `true` untuk file yang diterima.
    incoming: bool,
    peer_name: String,
    status: TransferStatus,
    /// Lokasi file setelah selesai.
    path: Option<String>,
    error: Option<String>,
}

/// Menerapkan satu event transfer dari node ke daftar yang ditampilkan UI.
pub fn apply(transfers: &mut Vec<TransferView>, event: TransferEvent) {
    match event {
        TransferEvent::Started {
            id,
            peer,
            name,
            size,
            incoming,
        } => transfers.push(TransferView {
            id,
            name,
            size,
            transferred: 0,
            incoming,
            peer_name: peer.name,
            status: TransferStatus::Active,
            path: None,
            error: None,
        }),
        TransferEvent::Progress { id, transferred } => {
            if let Some(transfer) = transfers.iter_mut().find(|transfer| transfer.id == id) {
                transfer.transferred = transferred;
            }
        }
        TransferEvent::Finished { id, outcome } => {
            let Some(transfer) = transfers.iter_mut().find(|transfer| transfer.id == id) else {
                return;
            };
            match outcome {
                Ok(path) => {
                    transfer.status = TransferStatus::Done;
                    transfer.transferred = transfer.size;
                    transfer.path = Some(path.to_string_lossy().into_owned());
                }
                Err(reason) => {
                    transfer.status = TransferStatus::Failed;
                    transfer.error = Some(reason);
                }
            }
        }
    }
}

#[tauri::command]
pub fn list_transfers(state: State<'_, AppState>) -> Vec<TransferView> {
    state.transfers.lock().unwrap().clone()
}

/// Mengirim satu atau beberapa file ke perangkat yang sudah di-pair.
#[tauri::command]
pub async fn send_files(state: State<'_, AppState>, fingerprint: String, paths: Vec<String>) -> CommandResult<()> {
    let peer = ensure_peer(&state, &fingerprint).await?;
    let mut failures = Vec::new();
    for path in paths {
        if let Err(error) = peer.send_file(PathBuf::from(&path)).await {
            let name = Path::new(&path)
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
            failures.push(format!("{name}: {error:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("Tidak bisa dikirim: {}", failures.join("; ")))
    }
}

#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, id: u64) {
    state.node.cancel_transfer(id);
}

/// Membuang transfer yang sudah selesai atau gagal dari daftar.
#[tauri::command]
pub fn clear_transfers(app: AppHandle, state: State<'_, AppState>) {
    state
        .transfers
        .lock()
        .unwrap()
        .retain(|transfer| transfer.status == TransferStatus::Active);
    let _ = app.emit(EVENT_TRANSFERS_CHANGED, ());
}

/// Argumen `explorer` untuk membuka folder berisi `path` dengan file itu
/// terpilih. Hanya path-nya yang dikutip: kalau seluruh argumen dikutip (yang
/// dilakukan `Command::arg` begitu ada spasi) atau path berkoma tidak dikutip,
/// Explorer membuka folder Dokumen. Nama file Windows tidak bisa memuat tanda
/// kutip, jadi tidak ada yang perlu di-escape.
#[cfg(any(target_os = "windows", test))]
fn explorer_select_argument(path: &str) -> String {
    format!("/select,\"{path}\"")
}

/// Menampilkan file hasil transfer di Finder atau Explorer.
#[tauri::command]
pub fn reveal_transfer(state: State<'_, AppState>, id: u64) -> CommandResult<()> {
    let path = state
        .transfers
        .lock()
        .unwrap()
        .iter()
        .find(|transfer| transfer.id == id)
        .and_then(|transfer| transfer.path.clone())
        .ok_or("File itu belum tersimpan.")?;

    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg("-R").arg(&path).status();
    #[cfg(target_os = "windows")]
    let opened = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(explorer_select_argument(&path))
            .status()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opened = std::process::Command::new("xdg-open")
        .arg(Path::new(&path).parent().unwrap_or(Path::new(".")))
        .status();

    opened.map(|_| ()).map_err(|error| format!("Gagal membuka folder: {error}"))
}

#[cfg(test)]
mod tests {
    use sambung_core::node::DeviceInfo;
    use sambung_core::trust::Os;

    use super::*;

    fn started(id: u64, incoming: bool) -> TransferEvent {
        TransferEvent::Started {
            id,
            peer: DeviceInfo {
                fingerprint: "ab".repeat(32).parse().unwrap(),
                name: "PC".to_owned(),
                os: Os::Windows,
            },
            name: "foto.jpg".to_owned(),
            size: 1000,
            incoming,
        }
    }

    #[test]
    fn transfer_goes_from_active_to_done_with_its_path() {
        let mut transfers = Vec::new();
        apply(&mut transfers, started(1, true));
        apply(&mut transfers, TransferEvent::Progress { id: 1, transferred: 400 });
        assert_eq!((transfers[0].status, transfers[0].transferred), (TransferStatus::Active, 400));

        apply(
            &mut transfers,
            TransferEvent::Finished {
                id: 1,
                outcome: Ok(PathBuf::from("/unduhan/foto.jpg")),
            },
        );

        assert_eq!(transfers[0].status, TransferStatus::Done);
        assert_eq!(transfers[0].transferred, 1000);
        assert_eq!(transfers[0].path.as_deref(), Some("/unduhan/foto.jpg"));
    }

    #[test]
    fn explorer_argument_quotes_only_the_path() {
        assert_eq!(
            explorer_select_argument(r"C:\Users\Budi Santoso\Downloads\Sambung\foto, liburan 1.jpg"),
            r#"/select,"C:\Users\Budi Santoso\Downloads\Sambung\foto, liburan 1.jpg""#
        );
        assert_eq!(explorer_select_argument(r"C:\unduhan\foto.jpg"), r#"/select,"C:\unduhan\foto.jpg""#);
    }

    #[test]
    fn failed_transfer_keeps_its_reason() {
        let mut transfers = Vec::new();
        apply(&mut transfers, started(1, false));
        apply(
            &mut transfers,
            TransferEvent::Finished {
                id: 1,
                outcome: Err("dibatalkan".to_owned()),
            },
        );
        assert_eq!(transfers[0].status, TransferStatus::Failed);
        assert_eq!(transfers[0].error.as_deref(), Some("dibatalkan"));
    }

    #[test]
    fn events_for_unknown_transfers_are_ignored() {
        let mut transfers = Vec::new();
        apply(&mut transfers, TransferEvent::Progress { id: 9, transferred: 1 });
        apply(
            &mut transfers,
            TransferEvent::Finished {
                id: 9,
                outcome: Err("x".to_owned()),
            },
        );
        assert!(transfers.is_empty());
    }
}

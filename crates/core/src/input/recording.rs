//! Injector tiruan yang hanya mencatat event, untuk uji mandiri sesi.

use std::sync::{Arc, Mutex};

use super::{InputBackend, InputInjector};
use crate::capture::DisplayInfo;
use crate::protocol::InputEvent;

/// Mencatat event yang "disuntikkan" tanpa menyentuh mouse atau keyboard sungguhan.
#[derive(Clone, Default)]
pub struct RecordingInput {
    events: Arc<Mutex<Vec<InputEvent>>>,
}

impl RecordingInput {
    /// Event yang sudah diterima sejauh ini, berurutan.
    pub fn events(&self) -> Vec<InputEvent> {
        self.events.lock().unwrap().clone()
    }
}

struct Recorder(Arc<Mutex<Vec<InputEvent>>>);

impl InputInjector for Recorder {
    fn inject(&mut self, event: &InputEvent, _display: &DisplayInfo) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }

    fn release_all(&mut self) {}
}

impl InputBackend for RecordingInput {
    fn open(&self) -> anyhow::Result<Box<dyn InputInjector>> {
        Ok(Box::new(Recorder(Arc::clone(&self.events))))
    }
}

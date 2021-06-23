use rtic_scope_api as api;
use anyhow::Result;

pub struct Dummy {}

impl api::Frontend for Dummy {
    fn spawn(rx: std::sync::mpsc::Receiver<api::EventChunk>) -> Result<std::thread::JoinHandle<Result<()>>> {
        Ok(std::thread::spawn(move || {
            for chunk in rx {
                drop(chunk);
            }

            // channel has hung when iter above fails

            Ok(())
        }))
    }
}

//! Per-document debounce; unrelated documents never hold a permit during backoff.
use std::{sync::Arc, time::Duration};

use crate::{DevopsError, DevopsService};

impl DevopsService {
    pub(crate) fn schedule_indexing(self: &Arc<Self>, actor: String, id: String) {
        let mut jobs = self.indexing_jobs.lock().expect("indexing registry poisoned");
        if let Some(job) = jobs.get(&id) {
            job.send_replace(actor);
            return;
        }
        let (sender, mut receiver) = tokio::sync::watch::channel(actor);
        jobs.insert(id.clone(), sender);
        let service = Arc::clone(self);
        tokio::spawn(async move {
            'latest: loop {
                // Restart the debounce whenever another save arrives.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {},
                    _ = receiver.changed() => { continue 'latest; }
                }
                let actor = receiver.borrow_and_update().clone();
                for attempt in 0..3 {
                    let result = {
                        let _permit = service
                            .indexing_slots
                            .acquire()
                            .await
                            .expect("indexing semaphore closed");
                        service.process_rag_document(&actor, &id).await
                    };
                    if receiver.has_changed().unwrap_or(false) {
                        receiver.borrow_and_update();
                        continue 'latest;
                    }
                    match result {
                        Ok(_)
                        | Err(DevopsError::BadRequest(_) | DevopsError::NotFound(_) | DevopsError::Forbidden(_)) => {
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(document_id = %id, attempt, %error, "automatic knowledge indexing failed");
                            if attempt == 2 {
                                break;
                            }
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(5 * (attempt + 1))) => {},
                                _ = receiver.changed() => { continue 'latest; }
                            }
                        }
                    }
                }
                // Check and remove under the same lock used by save scheduling.
                let changed = {
                    let mut jobs = service.indexing_jobs.lock().expect("indexing registry poisoned");
                    if receiver.has_changed().unwrap_or(false) {
                        receiver.borrow_and_update();
                        true
                    } else {
                        jobs.remove(&id);
                        false
                    }
                };
                if changed {
                    continue 'latest;
                }
                break;
            }
        });
    }
}

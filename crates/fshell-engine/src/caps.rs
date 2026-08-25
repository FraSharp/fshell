// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::CapPromptRequest;
use fshell_capabilities::CapsRegistry;

/// Capability registry, audit log, and prompt channel.
pub struct Caps {
    pub caps: Arc<RwLock<CapsRegistry>>,
    pub strict_mode_temp_count: Arc<std::sync::atomic::AtomicU32>,
    pub audit_log: Arc<Mutex<VecDeque<String>>>,
    pub cap_prompt_tx: Arc<tokio::sync::mpsc::Sender<CapPromptRequest>>,
    pub cap_prompt_rx: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<CapPromptRequest>>>>,
}

impl Clone for Caps {
    fn clone(&self) -> Self {
        Self {
            caps: self.caps.clone(),
            strict_mode_temp_count: self.strict_mode_temp_count.clone(),
            audit_log: self.audit_log.clone(),
            cap_prompt_tx: self.cap_prompt_tx.clone(),
            cap_prompt_rx: self.cap_prompt_rx.clone(),
        }
    }
}

impl std::fmt::Debug for Caps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Caps")
            .field("caps", &self.caps)
            .field("strict_mode_temp_count", &self.strict_mode_temp_count)
            .field("audit_log", &self.audit_log)
            .finish()
    }
}

//! Per-account signatures (stored locally).

use serde_json::json;

use super::Service;
use crate::error::{Error, Result};
use crate::model::{Signature, SignatureSettings};

const KEY: &str = "signatures";

impl Service {
    pub(crate) fn get_signatures(&self, account_id: &str) -> Result<SignatureSettings> {
        if let Some(s) = self.store.get_account_json::<SignatureSettings>(account_id, KEY)? {
            return Ok(s);
        }
        // Migration: the former global signature becomes the account's default.
        let legacy = std::fs::read_to_string(self.opts.data_dir.join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["signatureHtml"].as_str().map(str::to_string))
            .filter(|h| !h.trim().is_empty());
        let mut settings = SignatureSettings::default();
        if let Some(html) = legacy {
            let id = uuid::Uuid::new_v4().to_string();
            settings.signatures.push(Signature {
                id: id.clone(),
                name: "Standard".into(),
                html,
            });
            settings.default_id = Some(id);
        }
        Ok(settings)
    }

    pub(crate) fn save_signatures(&self, account_id: &str, mut s: SignatureSettings) -> Result<SignatureSettings> {
        if self.store.list_accounts()?.iter().all(|a| a.id != account_id) {
            return Err(Error::NotFound("Konto".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for (i, sig) in s.signatures.iter_mut().enumerate() {
            // Empty or duplicate ids (copied signature) get a fresh one.
            if sig.id.trim().is_empty() || !ids.insert(sig.id.clone()) {
                sig.id = uuid::Uuid::new_v4().to_string();
                ids.insert(sig.id.clone());
            }
            if sig.name.trim().is_empty() {
                sig.name = format!("Signatur {}", i + 1);
            }
        }
        let default_valid = s
            .default_id
            .as_ref()
            .is_some_and(|d| s.signatures.iter().any(|x| &x.id == d));
        if !default_valid {
            s.default_id = s.signatures.first().map(|x| x.id.clone());
        }
        self.store.put_account_json(account_id, KEY, &s)?;
        self.emit("signatures-changed", json!({"accountId": account_id}));
        Ok(s)
    }

    /// Default signature of an account for a compose mode, if one applies.
    pub(crate) fn signature_for(&self, account_id: &str, mode: &str) -> Option<Signature> {
        let s = self.get_signatures(account_id).ok()?;
        let applies = match mode {
            "new" => s.use_on_new,
            "reply" | "replyAll" => s.use_on_reply,
            "forward" => s.use_on_forward,
            _ => false,
        };
        if !applies {
            return None;
        }
        let id = s.default_id.as_ref()?;
        s.signatures.into_iter().find(|x| &x.id == id)
    }
}

/// HTML block inserted into the editor: an empty line to type into, then the
/// signature wrapped so the compose window can swap it.
pub(crate) fn signature_block(sig: Option<&Signature>) -> String {
    match sig {
        Some(s) if !s.html.trim().is_empty() => format!(
            "<div><br></div><div class=\"signature\" data-signature-id=\"{}\">{}</div>",
            html_escape::encode_double_quoted_attribute(&s.id),
            s.html
        ),
        _ => "<div><br></div>".into(),
    }
}

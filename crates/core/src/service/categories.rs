use serde_json::json;

use super::Service;
use crate::convert;
use crate::error::{Error, Result};
use crate::graph::seg;
use crate::model::Category;

const CACHE_KEY: &str = "categories";

impl Service {
    /// Last known categories without a server round trip (None = never loaded).
    pub(crate) fn cached_categories(&self, account_id: &str) -> Result<Option<Vec<Category>>> {
        if crate::imap::is_imap_id(account_id) {
            return self.imap_categories(account_id).map(Some);
        }
        self.store.get_account_json(account_id, CACHE_KEY)
    }

    pub(crate) async fn list_categories(&self, account_id: &str) -> Result<Vec<Category>> {
        if crate::imap::is_imap_id(account_id) {
            return self.imap_categories(account_id);
        }
        let c = self.client(account_id)?;
        match c.get_all("/me/outlook/masterCategories", &[], 1000).await {
            Ok(list) => {
                let cats: Vec<Category> = list.iter().map(convert::category).collect();
                self.store.put_account_json(account_id, CACHE_KEY, &cats)?;
                Ok(cats)
            }
            Err(e) if e.is_network() => Ok(self
                .store
                .get_account_json(account_id, CACHE_KEY)?
                .unwrap_or_default()),
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn create_category(&self, account_id: &str, name: &str, color: &str) -> Result<Category> {
        if crate::imap::is_imap_id(account_id) {
            return self.imap_create_category(account_id, name, color);
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Name der Kategorie fehlt."));
        }
        if name.chars().count() > 255 {
            return Err(Error::invalid("Der Name der Kategorie ist zu lang (max. 255 Zeichen)."));
        }
        validate_color(color)?;
        let c = self.client(account_id)?;
        let v = c
            .post(
                "/me/outlook/masterCategories",
                &json!({"displayName": name, "color": color}),
            )
            .await
            .map_err(|e| match e {
                Error::Graph { status: 409, .. } => {
                    Error::invalid(format!("Die Kategorie \"{name}\" existiert bereits."))
                }
                other => other,
            })?;
        self.categories_changed(account_id).await;
        Ok(convert::category(&v))
    }

    pub(crate) async fn update_category(&self, account_id: &str, id: &str, color: &str) -> Result<Category> {
        if crate::imap::is_imap_id(account_id) {
            return self.imap_update_category(account_id, id, color);
        }
        validate_color(color)?;
        let c = self.client(account_id)?;
        let path = format!("/me/outlook/masterCategories/{}", seg(id));
        let v = c.patch(&path, &json!({"color": color})).await?;
        let v = if v.is_object() { v } else { c.get(&path).await? };
        self.categories_changed(account_id).await;
        Ok(convert::category(&v))
    }

    pub(crate) async fn delete_category(&self, account_id: &str, id: &str) -> Result<()> {
        if crate::imap::is_imap_id(account_id) {
            return self.imap_delete_category(account_id, id);
        }
        let c = self.client(account_id)?;
        match c.delete(&format!("/me/outlook/masterCategories/{}", seg(id))).await {
            // Already deleted elsewhere: the goal is reached.
            Err(e) if e.is_not_found() => {}
            other => other?,
        }
        self.categories_changed(account_id).await;
        Ok(())
    }

    async fn categories_changed(&self, account_id: &str) {
        let _ = self.list_categories(account_id).await;
        self.emit("categories-changed", json!({"accountId": account_id}));
    }
}

fn validate_color(color: &str) -> Result<()> {
    let ok = color == "none"
        || color
            .strip_prefix("preset")
            .filter(|n| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| n <= 24);
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!("Ungültige Farbe: {color}")))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colors() {
        assert!(super::validate_color("preset0").is_ok());
        assert!(super::validate_color("preset24").is_ok());
        assert!(super::validate_color("none").is_ok());
        assert!(super::validate_color("preset25").is_err());
        assert!(super::validate_color("red").is_err());
        assert!(super::validate_color("preset+1").is_err());
        assert!(super::validate_color("preset").is_err());
        assert!(super::validate_color("preset007").is_err());
        assert!(super::validate_color("Preset1").is_err());
        assert!(super::validate_color("").is_err());
    }
}

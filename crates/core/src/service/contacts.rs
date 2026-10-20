//! Contacts: Outlook contacts of Microsoft 365 accounts (delta sync), local
//! contacts and automatically collected addresses of people you wrote to.

use serde_json::{json, Value};

use super::Service;
use crate::error::{Error, Result};
use crate::graph::seg;
use crate::model::*;

const CONTACT_SELECT: &str =
    "id,displayName,givenName,surname,emailAddresses,companyName,jobTitle,businessPhones,mobilePhone,homePhones,personalNotes";

fn delta_key(account_id: &str) -> String {
    format!("contactsdelta|{account_id}")
}

/// Contact ids are global: `<accountId>|<graph id>` for Outlook contacts.
fn o365_id(account_id: &str, graph_id: &str) -> String {
    format!("{account_id}|{graph_id}")
}

fn graph_id(id: &str) -> Option<&str> {
    id.split_once('|').map(|(_, g)| g)
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").trim().to_string()
}

pub(crate) fn contact_from_graph(v: &Value, account_id: &str) -> Contact {
    let mut phones = Vec::new();
    for n in v["businessPhones"].as_array().into_iter().flatten() {
        if let Some(n) = n.as_str().filter(|n| !n.trim().is_empty()) {
            phones.push(Phone { kind: "business".into(), number: n.trim().into() });
        }
    }
    if let Some(n) = v["mobilePhone"].as_str().filter(|n| !n.trim().is_empty()) {
        phones.push(Phone { kind: "mobile".into(), number: n.trim().into() });
    }
    for n in v["homePhones"].as_array().into_iter().flatten() {
        if let Some(n) = n.as_str().filter(|n| !n.trim().is_empty()) {
            phones.push(Phone { kind: "home".into(), number: n.trim().into() });
        }
    }
    let emails = v["emailAddresses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["address"].as_str())
        .filter(|a| !a.trim().is_empty())
        .map(|a| a.trim().to_string())
        .collect();
    let mut display = s(v, "displayName");
    if display.is_empty() {
        display = format!("{} {}", s(v, "givenName"), s(v, "surname")).trim().to_string();
    }
    Contact {
        id: o365_id(account_id, v["id"].as_str().unwrap_or("")),
        account_id: account_id.to_string(),
        source: "o365".into(),
        display_name: display,
        given_name: s(v, "givenName"),
        surname: s(v, "surname"),
        emails,
        company: s(v, "companyName"),
        job_title: s(v, "jobTitle"),
        phones,
        notes: s(v, "personalNotes"),
        read_only: false,
    }
}

fn contact_to_graph(c: &ContactInput) -> Value {
    let phones = |kind: &str| -> Vec<String> {
        c.phones
            .iter()
            .filter(|p| p.kind == kind && !p.number.trim().is_empty())
            .map(|p| p.number.trim().to_string())
            .collect()
    };
    json!({
        "displayName": display_name(c),
        "givenName": c.given_name.trim(),
        "surname": c.surname.trim(),
        "emailAddresses": c.emails.iter().filter(|e| !e.trim().is_empty())
            .map(|e| json!({"name": display_name(c), "address": e.trim()})).collect::<Vec<_>>(),
        "companyName": c.company.trim(),
        "jobTitle": c.job_title.trim(),
        "businessPhones": phones("business"),
        "mobilePhone": phones("mobile").into_iter().next(),
        "homePhones": phones("home"),
        "personalNotes": c.notes,
    })
}

fn display_name(c: &ContactInput) -> String {
    let d = c.display_name.trim();
    if !d.is_empty() {
        return d.to_string();
    }
    let n = format!("{} {}", c.given_name.trim(), c.surname.trim()).trim().to_string();
    if !n.is_empty() {
        return n;
    }
    c.emails.first().cloned().unwrap_or_default()
}

fn validate(c: &ContactInput) -> Result<()> {
    if display_name(c).is_empty() {
        return Err(Error::invalid("Bitte einen Namen oder eine E-Mail-Adresse angeben."));
    }
    for e in &c.emails {
        let e = e.trim();
        if !e.is_empty() && !e.contains('@') {
            return Err(Error::invalid(format!("Ungültige E-Mail-Adresse: {e}")));
        }
    }
    Ok(())
}

fn local_contact(id: String, account_id: &str, c: &ContactInput) -> Contact {
    Contact {
        id,
        account_id: account_id.to_string(),
        source: "local".into(),
        display_name: display_name(c),
        given_name: c.given_name.trim().into(),
        surname: c.surname.trim().into(),
        emails: c.emails.iter().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect(),
        company: c.company.trim().into(),
        job_title: c.job_title.trim().into(),
        phones: c.phones.iter().filter(|p| !p.number.trim().is_empty()).cloned().collect(),
        notes: c.notes.clone(),
        read_only: false,
    }
}

impl Service {
    fn is_o365_account(&self, account_id: &str) -> bool {
        !account_id.starts_with("imap:")
    }

    /// Delta sync of the account's Outlook contacts into the local cache.
    pub(crate) async fn sync_contacts(&self, account_id: &str) -> Result<SyncResult> {
        let mut result = SyncResult::default();
        if !self.is_o365_account(account_id) {
            return Ok(result);
        }
        let c = self.client(account_id)?;
        let mut url = self
            .store
            .kv_get(&delta_key(account_id))?
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| format!("/me/contacts/delta?$select={CONTACT_SELECT}"));
        let headers = [("Prefer", "odata.maxpagesize=200".to_string())];
        loop {
            let page = match c.get_h(&url, &headers).await {
                Err(Error::Graph { status: 400 | 410, .. }) if url.contains("deltatoken") => {
                    // Expired sync state: full resync.
                    self.store.kv_set(&delta_key(account_id), "")?;
                    url = format!("/me/contacts/delta?$select={CONTACT_SELECT}");
                    continue;
                }
                other => other?,
            };
            for item in page["value"].as_array().cloned().unwrap_or_default() {
                let gid = item["id"].as_str().unwrap_or("");
                if gid.is_empty() {
                    continue;
                }
                let id = o365_id(account_id, gid);
                if item.get("@removed").is_some() {
                    if self.store.get_contact(&id)?.is_some() {
                        self.store.delete_contact(&id)?;
                        result.removed += 1;
                    }
                    continue;
                }
                let contact = contact_from_graph(&item, account_id);
                match self.store.get_contact(&id)? {
                    Some(old) if old == contact => {}
                    Some(_) => result.updated += 1,
                    None => result.added += 1,
                }
                self.store.upsert_contact(&contact)?;
            }
            if let Some(next) = page["@odata.nextLink"].as_str() {
                url = next.to_string();
                continue;
            }
            if let Some(delta) = page["@odata.deltaLink"].as_str() {
                self.store.kv_set(&delta_key(account_id), delta)?;
            }
            break;
        }
        if result.added + result.updated + result.removed > 0 {
            self.emit("contacts-changed", json!({}));
        }
        Ok(result)
    }

    pub(crate) fn list_contacts(
        &self,
        query: Option<&str>,
        account_id: Option<&str>,
        source: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<ContactPage> {
        self.store.list_contacts(query, account_id, source, offset, limit)
    }

    pub(crate) fn get_contact(&self, id: &str) -> Result<Contact> {
        self.store
            .get_contact(id)?
            .ok_or_else(|| Error::NotFound("Kontakt".into()))
    }

    pub(crate) async fn create_contact(&self, account_id: &str, input: ContactInput) -> Result<Contact> {
        validate(&input)?;
        let contact = if self.is_o365_account(account_id) {
            let c = self.client(account_id)?;
            let v = c.post("/me/contacts", &contact_to_graph(&input)).await?;
            contact_from_graph(&v, account_id)
        } else {
            local_contact(format!("local:{}", uuid::Uuid::new_v4()), account_id, &input)
        };
        self.store.upsert_contact(&contact)?;
        self.remove_collected_duplicates(&contact)?;
        self.emit("contacts-changed", json!({}));
        Ok(contact)
    }

    pub(crate) async fn update_contact(&self, id: &str, input: ContactInput) -> Result<Contact> {
        validate(&input)?;
        let old = self.get_contact(id)?;
        let contact = match old.source.as_str() {
            "o365" => {
                let c = self.client(&old.account_id)?;
                let gid = graph_id(id).ok_or_else(|| Error::invalid("Ungültige Kontakt-ID"))?;
                let v = c
                    .patch(&format!("/me/contacts/{}", seg(gid)), &contact_to_graph(&input))
                    .await?;
                if v.is_object() && v["id"].is_string() {
                    contact_from_graph(&v, &old.account_id)
                } else {
                    contact_from_graph(&c.get(&format!("/me/contacts/{}?$select={CONTACT_SELECT}", seg(gid))).await?, &old.account_id)
                }
            }
            // Editing a collected address turns it into a real local contact.
            _ => {
                let new_id = if old.source == "collected" {
                    format!("local:{}", uuid::Uuid::new_v4())
                } else {
                    old.id.clone()
                };
                if new_id != old.id {
                    self.store.delete_contact(&old.id)?;
                }
                local_contact(new_id, &old.account_id, &input)
            }
        };
        self.store.upsert_contact(&contact)?;
        self.emit("contacts-changed", json!({}));
        Ok(contact)
    }

    pub(crate) async fn delete_contact(&self, id: &str) -> Result<()> {
        let old = self.get_contact(id)?;
        if old.source == "o365" {
            let c = self.client(&old.account_id)?;
            let gid = graph_id(id).ok_or_else(|| Error::invalid("Ungültige Kontakt-ID"))?;
            match c.delete(&format!("/me/contacts/{}", seg(gid))).await {
                Err(e) if !e.is_not_found() => return Err(e),
                _ => {}
            }
        }
        self.store.delete_contact(id)?;
        self.emit("contacts-changed", json!({}));
        Ok(())
    }

    fn remove_collected_duplicates(&self, contact: &Contact) -> Result<()> {
        for e in &contact.emails {
            self.store.delete_contact(&collected_id(&contact.account_id, e))?;
        }
        Ok(())
    }

    /// Remembers recipients of sent mails that are not yet known contacts.
    pub(crate) fn collect_addresses(&self, account_id: &str, addresses: &[Address]) -> Result<()> {
        let mut changed = false;
        for a in addresses {
            let addr = a.address.trim();
            if !addr.contains('@') || self.store.contact_with_email(addr)? {
                continue;
            }
            let name = if a.name.trim().is_empty() || a.name.trim() == addr {
                addr.to_string()
            } else {
                a.name.trim().to_string()
            };
            self.store.upsert_contact(&Contact {
                id: collected_id(account_id, addr),
                account_id: account_id.to_string(),
                source: "collected".into(),
                display_name: name,
                given_name: String::new(),
                surname: String::new(),
                emails: vec![addr.to_string()],
                company: String::new(),
                job_title: String::new(),
                phones: vec![],
                notes: String::new(),
                read_only: false,
            })?;
            changed = true;
        }
        if changed {
            self.emit("contacts-changed", json!({}));
        }
        Ok(())
    }

    /// Address suggestions from local contacts (name, any e-mail, company).
    pub(crate) fn contact_suggestions(&self, query: &str, limit: i64) -> Result<Vec<Address>> {
        let page = self.store.list_contacts(Some(query), None, None, 0, limit)?;
        let q = query.to_lowercase();
        let mut out = Vec::new();
        for c in page.items {
            // Prefer the address that matches the query, else the first one.
            let addr = c
                .emails
                .iter()
                .find(|e| e.to_lowercase().contains(&q))
                .or(c.emails.first());
            if let Some(addr) = addr {
                out.push(Address { name: c.display_name.clone(), address: addr.clone() });
            }
        }
        Ok(out)
    }
}

fn collected_id(account_id: &str, email: &str) -> String {
    format!("collected:{account_id}:{}", email.to_lowercase())
}

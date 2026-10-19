//! People search, master categories, mailbox settings.

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use crate::state::Mailbox;
use crate::util::{created, no_content, ok, paged, GErr, GResult, Req};

const COLORS: &[&str] = &[
    "none", "preset0", "preset1", "preset2", "preset3", "preset4", "preset5", "preset6", "preset7", "preset8", "preset9",
    "preset10", "preset11", "preset12", "preset13", "preset14", "preset15", "preset16", "preset17", "preset18",
    "preset19", "preset20", "preset21", "preset22", "preset23", "preset24",
];

pub fn handle(mb: &mut Mailbox, rest: &[&str], req: &Req) -> GResult {
    let m = &req.method;
    match rest {
        ["people"] if m == Method::GET => people(mb, req),
        ["outlook", "masterCategories"] if m == Method::GET => {
            let vals = mb.categories.clone();
            ok(json!({ "@odata.context": format!("{}/v1.0/$metadata#users('{}')/outlook/masterCategories", req.base_url, mb.addr), "value": vals }))
        }
        ["outlook", "masterCategories"] if m == Method::POST => {
            let body = req.json()?;
            let name = body["displayName"].as_str().unwrap_or("").trim().to_string();
            if name.is_empty() {
                return Err(GErr::bad("displayName is required."));
            }
            let color = body["color"].as_str().unwrap_or("none");
            if !COLORS.contains(&color) {
                return Err(GErr::bad(format!("Invalid category color '{color}'.")));
            }
            if mb.categories.iter().any(|c| c["displayName"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&name))) {
                return Err(GErr::new(StatusCode::CONFLICT, "ErrorDuplicateCategory", format!("A category named '{name}' already exists.")));
            }
            let c = json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": name, "color": color });
            mb.categories.push(c.clone());
            created(c)
        }
        ["outlook", "masterCategories", id] => {
            let pos = mb.categories.iter().position(|c| c["id"] == *id).ok_or_else(|| GErr::not_found("outlookCategory"))?;
            match *m {
                Method::GET => ok(mb.categories[pos].clone()),
                Method::PATCH => {
                    let body = req.json()?;
                    if body.get("displayName").is_some_and(|n| *n != mb.categories[pos]["displayName"]) {
                        return Err(GErr::bad("The displayName of a category cannot be changed."));
                    }
                    if let Some(color) = body["color"].as_str() {
                        if !COLORS.contains(&color) {
                            return Err(GErr::bad(format!("Invalid category color '{color}'.")));
                        }
                        mb.categories[pos]["color"] = json!(color);
                    }
                    ok(mb.categories[pos].clone())
                }
                Method::DELETE => {
                    mb.categories.remove(pos);
                    no_content()
                }
                _ => Err(GErr::bad("Unsupported method")),
            }
        }
        ["mailboxSettings"] if m == Method::GET => ok(json!({
            "@odata.context": format!("{}/v1.0/$metadata#users('{}')/mailboxSettings", req.base_url, mb.addr),
            "archiveFolder": mb.folder_id("archive"),
            "timeZone": "W. Europe Standard Time",
            "delegateMeetingMessageDeliveryOptions": "sendToDelegateOnly",
            "dateFormat": "dd.MM.yyyy",
            "timeFormat": "HH:mm",
            "userPurpose": if mb.is_primary { "user" } else { "shared" },
            "automaticRepliesSetting": mb.auto_reply,
            "language": { "locale": "de-DE", "displayName": "Deutsch (Deutschland)" },
            "workingHours": {
                "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
                "startTime": "08:00:00.0000000",
                "endTime": "17:00:00.0000000",
                "timeZone": { "name": "W. Europe Standard Time" }
            }
        })),
        ["mailboxSettings", "automaticRepliesSetting"] if m == Method::GET => {
            let mut v = mb.auto_reply.clone();
            v["@odata.context"] = json!(format!("{}/v1.0/$metadata#users('{}')/mailboxSettings/automaticRepliesSetting", req.base_url, mb.addr));
            ok(v)
        }
        ["mailboxSettings"] if m == Method::PATCH => {
            let body = req.json()?;
            if let Some(ar) = body.get("automaticRepliesSetting") {
                if let Some(st) = ar["status"].as_str() {
                    if !["disabled", "alwaysEnabled", "scheduled"].contains(&st) {
                        return Err(GErr::bad(format!("Invalid automaticRepliesSetting.status '{st}'.")));
                    }
                }
                if let Some(aud) = ar["externalAudience"].as_str() {
                    if !["none", "contactsOnly", "all"].contains(&aud) {
                        return Err(GErr::bad(format!("Invalid externalAudience '{aud}'.")));
                    }
                }
                for k in ["scheduledStartDateTime", "scheduledEndDateTime"] {
                    if let Some(dt) = ar.get(k).filter(|v| !v.is_null()) {
                        if dt["dateTime"].as_str().and_then(crate::util::parse_naive).is_none() {
                            return Err(GErr::bad(format!("{k}.dateTime is invalid.")));
                        }
                    }
                }
                crate::util::merge(&mut mb.auto_reply, ar);
            }
            ok(json!({
                "@odata.context": format!("{}/v1.0/$metadata#Me/mailboxSettings", req.base_url),
                "automaticRepliesSetting": mb.auto_reply,
            }))
        }
        _ => Err(GErr::new(StatusCode::BAD_REQUEST, "BadRequest", format!("Unsupported request: {} /{}", req.method, rest.join("/")))),
    }
}

fn people(mb: &Mailbox, req: &Req) -> GResult {
    let q = req.q("search").map(|s| s.trim().trim_matches('"').to_lowercase()).unwrap_or_default();
    let vals: Vec<Value> = mb
        .people
        .iter()
        .filter(|p| {
            if q.is_empty() {
                return true;
            }
            let name = p["displayName"].as_str().unwrap_or("").to_lowercase();
            let addr = p.pointer("/scoredEmailAddresses/0/address").and_then(Value::as_str).unwrap_or("").to_lowercase();
            name.split_whitespace().any(|w| w.starts_with(&q)) || name.starts_with(&q) || addr.starts_with(&q)
        })
        .cloned()
        .collect();
    ok(paged(req, vals, 10))
}

/// Builds a `person` entity for the seed.
pub fn person(name: &str, addr: &str, company: &str, title: &str) -> Value {
    let (given, sur) = name.split_once(' ').unwrap_or((name, ""));
    json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "displayName": name,
        "givenName": given,
        "surname": sur,
        "birthday": "",
        "personNotes": "",
        "isFavorite": false,
        "jobTitle": title,
        "companyName": company,
        "yomiCompany": "",
        "department": "",
        "officeLocation": null,
        "profession": "",
        "userPrincipalName": addr,
        "imAddress": null,
        "scoredEmailAddresses": [{ "address": addr, "relevanceScore": 8.0, "selectionLikelihood": "notSpecified" }],
        "phones": [],
        "personType": { "class": "Person", "subclass": if addr.ends_with("@contoso.example") { "OrganizationUser" } else { "PersonalContact" } }
    })
}

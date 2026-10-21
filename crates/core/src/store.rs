//! Local SQLite cache: accounts, folders, message summaries, bodies, calendar
//! data. Lets the UI sort/filter instantly and work offline.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::model::*;

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE TABLE IF NOT EXISTS mailboxes (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    address TEXT NOT NULL,
    display_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    created INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE TABLE IF NOT EXISTS folders (
    mailbox_id TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    parent_id TEXT,
    display_name TEXT NOT NULL,
    well_known TEXT,
    unread INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    child_count INTEGER NOT NULL DEFAULT 0,
    delta_link TEXT,
    fully_loaded INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (mailbox_id, id)
);
CREATE TABLE IF NOT EXISTS messages (
    mailbox_id TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    folder_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL DEFAULT '',
    subject TEXT NOT NULL DEFAULT '',
    from_name TEXT NOT NULL DEFAULT '',
    from_addr TEXT NOT NULL DEFAULT '',
    to_json TEXT NOT NULL DEFAULT '[]',
    received TEXT NOT NULL DEFAULT '',
    is_read INTEGER NOT NULL DEFAULT 1,
    is_draft INTEGER NOT NULL DEFAULT 0,
    importance TEXT NOT NULL DEFAULT 'normal',
    flag TEXT NOT NULL DEFAULT 'notFlagged',
    has_att INTEGER NOT NULL DEFAULT 0,
    categories_json TEXT NOT NULL DEFAULT '[]',
    preview TEXT NOT NULL DEFAULT '',
    due_at TEXT,
    PRIMARY KEY (mailbox_id, id)
);
CREATE INDEX IF NOT EXISTS messages_folder ON messages(mailbox_id, folder_id, received);
CREATE TABLE IF NOT EXISTS bodies (
    mailbox_id TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    PRIMARY KEY (mailbox_id, id)
);
CREATE TABLE IF NOT EXISTS account_json (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (account_id, kind)
);
CREATE TABLE IF NOT EXISTS events (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    calendar_id TEXT NOT NULL,
    id TEXT NOT NULL,
    start TEXT NOT NULL,
    end TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (account_id, calendar_id, id)
);
CREATE INDEX IF NOT EXISTS events_range ON events(account_id, calendar_id, start, end);
CREATE TABLE IF NOT EXISTS contacts (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    source TEXT NOT NULL,
    display_name TEXT NOT NULL,
    search TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS contacts_name ON contacts(display_name COLLATE NOCASE);
CREATE TABLE IF NOT EXISTS kv (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// `%…%` LIKE pattern matching `s` literally; use with `ESCAPE '\\'`.
fn like_contains(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Outlook folder order for well-known folders.
fn well_known_rank(w: Option<&str>) -> i32 {
    match w {
        Some("inbox") => 0,
        Some("snoozed") => 1,
        Some("drafts") => 2,
        Some("sentitems") => 3,
        Some("deleteditems") => 4,
        Some("archive") => 5,
        Some("junkemail") => 6,
        Some("outbox") => 7,
        _ => 100,
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        // Migrations for caches created by older versions.
        let has_due: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name='due_at'")?
            .exists([])?;
        if !has_due {
            conn.execute_batch("ALTER TABLE messages ADD COLUMN due_at TEXT")?;
        }
        let has_order: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='sort_order'")?
            .exists([])?;
        if !has_order {
            conn.execute_batch(
                "ALTER TABLE accounts ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE accounts ADD COLUMN avatar TEXT;",
            )?;
        }
        let has_label: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='label'")?
            .exists([])?;
        if !has_label {
            conn.execute_batch(
                "ALTER TABLE accounts ADD COLUMN label TEXT;
                 ALTER TABLE accounts ADD COLUMN badge_color TEXT;
                 ALTER TABLE accounts ADD COLUMN badge_text TEXT;",
            )?;
        }
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- accounts & mailboxes ----

    pub fn upsert_account(&self, id: &str, email: &str, display_name: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO accounts(id,email,display_name) VALUES(?1,?2,?3)
             ON CONFLICT(id) DO UPDATE SET email=excluded.email, display_name=excluded.display_name",
            params![id, email, display_name],
        )?;
        Ok(())
    }

    /// Display order of the accounts: `ids` from top to bottom (unknown ids ignored).
    pub fn set_account_order(&self, ids: &[String]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE accounts SET sort_order=?2 WHERE id=?1", params![id, i as i64 + 1])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_account_avatar(&self, id: &str, avatar: Option<&str>) -> Result<bool> {
        Ok(self.c().execute("UPDATE accounts SET avatar=?2 WHERE id=?1", params![id, avatar])? > 0)
    }

    /// Name in the views and look of the initials badge (None = default).
    pub fn set_account_look(&self, id: &str, label: Option<&str>, badge: &AccountBadge) -> Result<bool> {
        Ok(self.c().execute(
            "UPDATE accounts SET label=?2, badge_color=?3, badge_text=?4 WHERE id=?1",
            params![id, label, badge.color, badge.text],
        )? > 0)
    }

    pub fn delete_account(&self, id: &str) -> Result<()> {
        self.c().execute("DELETE FROM accounts WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn upsert_mailbox(&self, m: &Mailbox) -> Result<()> {
        self.c().execute(
            "INSERT INTO mailboxes(id,account_id,address,display_name,kind) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET address=excluded.address, display_name=excluded.display_name",
            params![m.id, m.account_id, m.address, m.display_name, m.kind],
        )?;
        Ok(())
    }

    pub fn delete_mailbox(&self, id: &str) -> Result<()> {
        self.c().execute("DELETE FROM mailboxes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let c = self.c();
        // sort_order 0 = never arranged: after the arranged ones, oldest first.
        let mut st = c.prepare(
            "SELECT id,email,display_name,avatar,label,badge_color,badge_text FROM accounts
             ORDER BY CASE sort_order WHEN 0 THEN 1 ELSE 0 END, sort_order, created, email",
        )?;
        let mut accounts: Vec<Account> = st
            .query_map([], |r| {
                let id: String = r.get(0)?;
                Ok(Account {
                    kind: if crate::imap::is_imap_id(&id) { "imap" } else { "o365" }.into(),
                    id,
                    email: r.get(1)?,
                    display_name: r.get(2)?,
                    mailboxes: vec![],
                    avatar: r.get(3)?,
                    label: r.get(4)?,
                    badge: {
                        let badge = AccountBadge { color: r.get(5)?, text: r.get(6)? };
                        (badge != AccountBadge::default()).then_some(badge)
                    },
                    auth: String::new(),
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut st = c.prepare(
            "SELECT id,account_id,address,display_name,kind FROM mailboxes
             ORDER BY CASE kind WHEN 'primary' THEN 0 ELSE 1 END, created, address",
        )?;
        let mbs = st.query_map([], mailbox_row)?;
        for m in mbs {
            let m = m?;
            if let Some(a) = accounts.iter_mut().find(|a| a.id == m.account_id) {
                a.mailboxes.push(m);
            }
        }
        Ok(accounts)
    }

    pub fn get_mailbox(&self, id: &str) -> Result<Mailbox> {
        self.c()
            .query_row(
                "SELECT id,account_id,address,display_name,kind FROM mailboxes WHERE id=?1",
                params![id],
                mailbox_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Postfach {id}")))
    }

    // ---- folders ----

    /// Replaces the folder list of a mailbox, keeping delta state of folders
    /// that still exist and dropping messages of folders that disappeared.
    pub fn replace_folders(&self, mailbox_id: &str, folders: &[Folder]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let existing: Vec<String> = {
            let mut st = tx.prepare("SELECT id FROM folders WHERE mailbox_id=?1")?;
            let rows = st.query_map(params![mailbox_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for f in folders {
            tx.execute(
                "INSERT INTO folders(mailbox_id,id,parent_id,display_name,well_known,unread,total,child_count)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(mailbox_id,id) DO UPDATE SET parent_id=excluded.parent_id,
                   display_name=excluded.display_name, well_known=excluded.well_known,
                   unread=excluded.unread, total=excluded.total, child_count=excluded.child_count",
                params![
                    mailbox_id,
                    f.id,
                    f.parent_id,
                    f.display_name,
                    f.well_known,
                    f.unread_count,
                    f.total_count,
                    f.child_folder_count
                ],
            )?;
        }
        for id in existing {
            if !folders.iter().any(|f| f.id == id) {
                tx.execute(
                    "DELETE FROM folders WHERE mailbox_id=?1 AND id=?2",
                    params![mailbox_id, id],
                )?;
                tx.execute(
                    "DELETE FROM messages WHERE mailbox_id=?1 AND folder_id=?2",
                    params![mailbox_id, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_folder(&self, f: &Folder) -> Result<()> {
        self.replace_one_folder(f)
    }

    fn replace_one_folder(&self, f: &Folder) -> Result<()> {
        self.c().execute(
            "INSERT INTO folders(mailbox_id,id,parent_id,display_name,well_known,unread,total,child_count)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(mailbox_id,id) DO UPDATE SET parent_id=excluded.parent_id,
               display_name=excluded.display_name, well_known=excluded.well_known,
               unread=excluded.unread, total=excluded.total, child_count=excluded.child_count",
            params![
                f.mailbox_id,
                f.id,
                f.parent_id,
                f.display_name,
                f.well_known,
                f.unread_count,
                f.total_count,
                f.child_folder_count
            ],
        )?;
        Ok(())
    }

    pub fn delete_folder(&self, mailbox_id: &str, id: &str) -> Result<()> {
        let c = self.c();
        c.execute(
            "DELETE FROM folders WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, id],
        )?;
        c.execute(
            "DELETE FROM messages WHERE mailbox_id=?1 AND folder_id=?2",
            params![mailbox_id, id],
        )?;
        Ok(())
    }

    pub fn list_folders(&self, mailbox_id: &str) -> Result<Vec<Folder>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id,mailbox_id,parent_id,display_name,well_known,unread,total,child_count
             FROM folders WHERE mailbox_id=?1",
        )?;
        let mut v: Vec<Folder> = st
            .query_map(params![mailbox_id], folder_row)?
            .collect::<std::result::Result<_, _>>()?;
        v.sort_by(|a, b| {
            well_known_rank(a.well_known.as_deref())
                .cmp(&well_known_rank(b.well_known.as_deref()))
                .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()))
        });
        Ok(v)
    }

    pub fn get_folder(&self, mailbox_id: &str, id: &str) -> Result<Option<Folder>> {
        Ok(self
            .c()
            .query_row(
                "SELECT id,mailbox_id,parent_id,display_name,well_known,unread,total,child_count
                 FROM folders WHERE mailbox_id=?1 AND id=?2",
                params![mailbox_id, id],
                folder_row,
            )
            .optional()?)
    }

    pub fn folder_by_well_known(&self, mailbox_id: &str, wk: &str) -> Result<Option<Folder>> {
        Ok(self
            .c()
            .query_row(
                "SELECT id,mailbox_id,parent_id,display_name,well_known,unread,total,child_count
                 FROM folders WHERE mailbox_id=?1 AND well_known=?2",
                params![mailbox_id, wk],
                folder_row,
            )
            .optional()?)
    }

    pub fn delta_link(&self, mailbox_id: &str, folder_id: &str) -> Result<Option<String>> {
        Ok(self
            .c()
            .query_row(
                "SELECT delta_link FROM folders WHERE mailbox_id=?1 AND id=?2",
                params![mailbox_id, folder_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_delta_link(&self, mailbox_id: &str, folder_id: &str, link: Option<&str>) -> Result<()> {
        self.c().execute(
            "UPDATE folders SET delta_link=?3 WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, folder_id, link],
        )?;
        Ok(())
    }

    pub fn set_fully_loaded(&self, mailbox_id: &str, folder_id: &str, v: bool) -> Result<()> {
        self.c().execute(
            "UPDATE folders SET fully_loaded=?3 WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, folder_id, v as i64],
        )?;
        Ok(())
    }

    pub fn is_fully_loaded(&self, mailbox_id: &str, folder_id: &str) -> Result<bool> {
        Ok(self
            .c()
            .query_row(
                "SELECT fully_loaded FROM folders WHERE mailbox_id=?1 AND id=?2",
                params![mailbox_id, folder_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            != 0)
    }

    pub fn adjust_folder_counts(&self, mailbox_id: &str, folder_id: &str, unread: i64, total: i64) -> Result<()> {
        self.c().execute(
            "UPDATE folders SET unread=MAX(0,unread+?3), total=MAX(0,total+?4) WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, folder_id, unread, total],
        )?;
        Ok(())
    }

    // ---- messages ----

    pub fn upsert_messages(&self, msgs: &[MessageSummary]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO messages(mailbox_id,id,folder_id,conversation_id,subject,from_name,from_addr,
                   to_json,received,is_read,is_draft,importance,flag,has_att,categories_json,preview,due_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(mailbox_id,id) DO UPDATE SET folder_id=excluded.folder_id,
                   conversation_id=excluded.conversation_id, subject=excluded.subject,
                   from_name=excluded.from_name, from_addr=excluded.from_addr, to_json=excluded.to_json,
                   received=excluded.received, is_read=excluded.is_read, is_draft=excluded.is_draft,
                   importance=excluded.importance, flag=excluded.flag, has_att=excluded.has_att,
                   categories_json=excluded.categories_json, preview=excluded.preview, due_at=excluded.due_at",
            )?;
            for m in msgs {
                let (fname, faddr) = m
                    .from
                    .as_ref()
                    .map(|a| (a.name.clone(), a.address.clone()))
                    .unwrap_or_default();
                st.execute(params![
                    m.mailbox_id,
                    m.id,
                    m.folder_id,
                    m.conversation_id,
                    m.subject,
                    fname,
                    faddr,
                    serde_json::to_string(&m.to)?,
                    m.received_at,
                    m.is_read,
                    m.is_draft,
                    m.importance,
                    m.flag,
                    m.has_attachments,
                    serde_json::to_string(&m.categories)?,
                    m.preview,
                    m.due_at
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn message_exists(&self, mailbox_id: &str, id: &str) -> Result<bool> {
        Ok(self
            .c()
            .query_row(
                "SELECT 1 FROM messages WHERE mailbox_id=?1 AND id=?2",
                params![mailbox_id, id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn delete_message(&self, mailbox_id: &str, id: &str) -> Result<()> {
        let c = self.c();
        c.execute(
            "DELETE FROM messages WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, id],
        )?;
        c.execute(
            "DELETE FROM bodies WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, id],
        )?;
        Ok(())
    }

    /// Ids of a folder's cached messages received at or after `since` (RFC3339 UTC).
    pub fn message_ids_since(&self, mailbox_id: &str, folder_id: &str, since: &str) -> Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id FROM messages WHERE mailbox_id=?1 AND folder_id=?2 AND received >= ?3")?;
        let rows = st.query_map(params![mailbox_id, folder_id, since], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Cached messages of a folder without a receive time – rows built from an
    /// incomplete server answer, to be fetched again.
    pub fn incomplete_message_ids(&self, mailbox_id: &str, folder_id: &str, limit: i64) -> Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id FROM messages WHERE mailbox_id=?1 AND folder_id=?2 AND received='' LIMIT ?3")?;
        let rows = st.query_map(params![mailbox_id, folder_id, limit], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_folder_messages(&self, mailbox_id: &str, folder_id: &str) -> Result<()> {
        self.c().execute(
            "DELETE FROM messages WHERE mailbox_id=?1 AND folder_id=?2",
            params![mailbox_id, folder_id],
        )?;
        Ok(())
    }

    pub fn get_summary(&self, mailbox_id: &str, id: &str) -> Result<Option<MessageSummary>> {
        Ok(self
            .c()
            .query_row(
                &format!("SELECT {MSG_COLS} FROM messages WHERE mailbox_id=?1 AND id=?2"),
                params![mailbox_id, id],
                message_row,
            )
            .optional()?)
    }

    pub fn folder_message_ids(&self, mailbox_id: &str, folder_id: &str, unread_only: bool) -> Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id FROM messages WHERE mailbox_id=?1 AND folder_id=?2 AND (?3=0 OR is_read=0)",
        )?;
        let rows = st.query_map(params![mailbox_id, folder_id, unread_only as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn oldest_received(&self, mailbox_id: &str, folder_id: &str) -> Result<Option<String>> {
        Ok(self
            .c()
            .query_row(
                "SELECT MIN(received) FROM messages WHERE mailbox_id=?1 AND folder_id=?2",
                params![mailbox_id, folder_id],
                |r| r.get::<_, Option<String>>(0),
            )?)
    }

    pub fn list_messages(&self, q: &ListQuery) -> Result<MessagePage> {
        let order_col = match q.sort_by.as_str() {
            "from" => "from_name COLLATE NOCASE",
            "subject" => "subject COLLATE NOCASE",
            "importance" => "CASE importance WHEN 'high' THEN 2 WHEN 'normal' THEN 1 ELSE 0 END",
            "flag" => "CASE flag WHEN 'flagged' THEN 2 WHEN 'complete' THEN 1 ELSE 0 END",
            "attachments" => "has_att",
            "categories" => "categories_json",
            _ => "received",
        };
        let dir = if q.sort_desc { "DESC" } else { "ASC" };
        let mut where_sql = String::from("mailbox_id=?1 AND folder_id=?2");
        if q.unread_only {
            where_sql.push_str(" AND is_read=0");
        }
        if q.flagged_only {
            where_sql.push_str(" AND flag='flagged'");
        }
        let cat_pattern = q
            .category
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| like_contains(&serde_json::to_string(c).unwrap_or_default()));
        if cat_pattern.is_some() {
            where_sql.push_str(" AND categories_json LIKE ?3 ESCAPE '\\'");
        } else {
            where_sql.push_str(" AND ?3 IS NULL");
        }
        let text_pattern = q
            .text
            .as_ref()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(like_contains);
        if text_pattern.is_some() {
            where_sql.push_str(
                " AND (subject LIKE ?4 ESCAPE '\\' OR from_name LIKE ?4 ESCAPE '\\' OR from_addr LIKE ?4 ESCAPE '\\' OR preview LIKE ?4 ESCAPE '\\' OR to_json LIKE ?4 ESCAPE '\\')",
            );
        } else {
            where_sql.push_str(" AND ?4 IS NULL");
        }
        let c = self.c();
        let (total, unread): (i64, i64) = c.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(is_read=0),0) FROM messages WHERE {where_sql}"),
            params![q.mailbox_id, q.folder_id, cat_pattern, text_pattern],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let limit = if q.limit <= 0 { 200 } else { q.limit };
        let mut st = c.prepare(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE {where_sql}
             ORDER BY {order_col} {dir}, received DESC LIMIT ?5 OFFSET ?6"
        ))?;
        let items = st
            .query_map(
                params![q.mailbox_id, q.folder_id, cat_pattern, text_pattern, limit, q.offset.max(0)],
                message_row,
            )?
            .collect::<std::result::Result<_, _>>()?;
        Ok(MessagePage {
            items,
            total,
            unread,
            can_load_older: false,
        })
    }

    /// Known correspondents (senders in the cache) matching `q`.
    pub fn search_addresses(&self, q: &str, limit: i64) -> Result<Vec<Address>> {
        let pattern = like_contains(q);
        let c = self.c();
        let mut st = c.prepare(
            "SELECT from_name, from_addr, COUNT(*) AS n FROM messages
             WHERE from_addr <> '' AND (from_name LIKE ?1 ESCAPE '\\' OR from_addr LIKE ?1 ESCAPE '\\')
             GROUP BY lower(from_addr) ORDER BY n DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![pattern, limit], |r| {
            Ok(Address {
                name: r.get(0)?,
                address: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Messages of a mailbox (any folder) carrying a category.
    pub fn messages_with_category(&self, mailbox_id: &str, category: &str) -> Result<Vec<MessageSummary>> {
        let pattern = like_contains(&serde_json::to_string(category)?);
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE mailbox_id=?1 AND categories_json LIKE ?2 ESCAPE '\\' ORDER BY received DESC"
        ))?;
        let rows = st.query_map(params![mailbox_id, pattern], message_row)?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|m| m.categories.iter().any(|x| x == category))
            .collect())
    }

    /// Messages of a mailbox (any folder) with a category starting with `prefix`.
    pub fn messages_with_category_prefix(&self, mailbox_id: &str, prefix: &str) -> Result<Vec<MessageSummary>> {
        let quoted = serde_json::to_string(prefix)?;
        let pattern = like_contains(quoted.trim_end_matches('"'));
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE mailbox_id=?1 AND categories_json LIKE ?2 ESCAPE '\\' ORDER BY received DESC"
        ))?;
        let rows = st.query_map(params![mailbox_id, pattern], message_row)?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|m| m.categories.iter().any(|x| x.starts_with(prefix)))
            .collect())
    }

    /// Messages of a mailbox with a due time before `ts` (RFC3339 UTC).
    pub fn messages_due_before(&self, mailbox_id: &str, ts: &str) -> Result<Vec<MessageSummary>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE mailbox_id=?1 AND due_at IS NOT NULL AND due_at < ?2 ORDER BY due_at"
        ))?;
        let rows = st.query_map(params![mailbox_id, ts], message_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ---- message bodies ----

    pub fn put_detail(&self, d: &MessageDetail) -> Result<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO bodies(mailbox_id,id,detail_json) VALUES(?1,?2,?3)",
            params![d.summary.mailbox_id, d.summary.id, serde_json::to_string(d)?],
        )?;
        Ok(())
    }

    pub fn get_detail(&self, mailbox_id: &str, id: &str) -> Result<Option<MessageDetail>> {
        let json: Option<String> = self
            .c()
            .query_row(
                "SELECT detail_json FROM bodies WHERE mailbox_id=?1 AND id=?2",
                params![mailbox_id, id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match json {
            Some(j) => serde_json::from_str(&j).ok(),
            None => None,
        })
    }

    pub fn delete_detail(&self, mailbox_id: &str, id: &str) -> Result<()> {
        self.c().execute(
            "DELETE FROM bodies WHERE mailbox_id=?1 AND id=?2",
            params![mailbox_id, id],
        )?;
        Ok(())
    }

    // ---- per-account JSON blobs (categories, calendars) ----

    pub fn put_account_json<T: serde::Serialize>(&self, account_id: &str, kind: &str, v: &T) -> Result<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO account_json(account_id,kind,json) VALUES(?1,?2,?3)",
            params![account_id, kind, serde_json::to_string(v)?],
        )?;
        Ok(())
    }

    pub fn get_account_json<T: serde::de::DeserializeOwned>(&self, account_id: &str, kind: &str) -> Result<Option<T>> {
        let json: Option<String> = self
            .c()
            .query_row(
                "SELECT json FROM account_json WHERE account_id=?1 AND kind=?2",
                params![account_id, kind],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    // ---- events ----

    /// Replaces the cached events of a calendar within [start, end).
    pub fn replace_events(&self, account_id: &str, calendar_id: &str, start: &str, end: &str, events: &[Event]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM events WHERE account_id=?1 AND calendar_id=?2 AND start < ?4 AND end > ?3",
            params![account_id, calendar_id, start, end],
        )?;
        for e in events {
            tx.execute(
                "INSERT OR REPLACE INTO events(account_id,calendar_id,id,start,end,json) VALUES(?1,?2,?3,?4,?5,?6)",
                params![account_id, calendar_id, e.id, e.start, e.end, serde_json::to_string(e)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn cached_events(&self, account_id: &str, calendar_id: &str, start: &str, end: &str) -> Result<Vec<Event>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT json FROM events WHERE account_id=?1 AND calendar_id=?2 AND start < ?4 AND end > ?3 ORDER BY start",
        )?;
        let rows = st.query_map(params![account_id, calendar_id, start, end], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for j in rows {
            if let Ok(e) = serde_json::from_str(&j?) {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn event_calendar(&self, account_id: &str, id: &str) -> Result<Option<String>> {
        Ok(self
            .c()
            .query_row(
                "SELECT calendar_id FROM events WHERE account_id=?1 AND id=?2 LIMIT 1",
                params![account_id, id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn delete_event(&self, account_id: &str, id: &str) -> Result<()> {
        self.c().execute(
            "DELETE FROM events WHERE account_id=?1 AND id=?2",
            params![account_id, id],
        )?;
        Ok(())
    }

    // ---- contacts ----

    pub fn upsert_contact(&self, c: &Contact) -> Result<()> {
        let search = format!(
            "{} {} {} {} {}",
            c.display_name,
            c.given_name,
            c.surname,
            c.emails.join(" "),
            c.company
        )
        .to_lowercase();
        self.c().execute(
            "INSERT OR REPLACE INTO contacts(id,account_id,source,display_name,search,json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![c.id, c.account_id, c.source, c.display_name, search, serde_json::to_string(c)?],
        )?;
        Ok(())
    }

    pub fn delete_contact(&self, id: &str) -> Result<()> {
        self.c().execute("DELETE FROM contacts WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn delete_account_contacts(&self, account_id: &str) -> Result<()> {
        self.c()
            .execute("DELETE FROM contacts WHERE account_id=?1", params![account_id])?;
        Ok(())
    }

    pub fn get_contact(&self, id: &str) -> Result<Option<Contact>> {
        let json: Option<String> = self
            .c()
            .query_row("SELECT json FROM contacts WHERE id=?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Contacts matching all words of `query` (name, emails, company).
    /// `source` filters by "o365" | "local" | "collected".
    pub fn list_contacts(
        &self,
        query: Option<&str>,
        account_id: Option<&str>,
        source: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<ContactPage> {
        let words: Vec<String> = query
            .unwrap_or("")
            .split_whitespace()
            .map(|w| like_contains(&w.to_lowercase()))
            .collect();
        let mut sql = String::from("FROM contacts WHERE (?1 IS NULL OR account_id=?1) AND (?2 IS NULL OR source=?2)");
        for i in 0..words.len() {
            sql.push_str(&format!(" AND search LIKE ?{} ESCAPE '\\'", i + 3));
        }
        let mut values: Vec<rusqlite::types::Value> = vec![
            account_id.map(|s| s.to_string().into()).unwrap_or(rusqlite::types::Value::Null),
            source.map(|s| s.to_string().into()).unwrap_or(rusqlite::types::Value::Null),
        ];
        values.extend(words.into_iter().map(rusqlite::types::Value::from));
        let c = self.c();
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) {sql}"),
            rusqlite::params_from_iter(values.iter()),
            |r| r.get(0),
        )?;
        let limit = if limit <= 0 { 500 } else { limit };
        let mut st = c.prepare(&format!(
            "SELECT json {sql} ORDER BY CASE source WHEN 'collected' THEN 1 ELSE 0 END, display_name COLLATE NOCASE LIMIT {limit} OFFSET {}",
            offset.max(0)
        ))?;
        let rows = st.query_map(rusqlite::params_from_iter(values.iter()), |r| r.get::<_, String>(0))?;
        let mut items = Vec::new();
        for j in rows {
            if let Ok(ct) = serde_json::from_str(&j?) {
                items.push(ct);
            }
        }
        Ok(ContactPage { items, total })
    }

    /// True if any contact already has this e-mail address.
    pub fn contact_with_email(&self, email: &str) -> Result<bool> {
        // LIKE only pre-filters (substring): check every candidate, not just the first.
        let c = self.c();
        let mut st = c.prepare("SELECT json FROM contacts WHERE search LIKE ?1 ESCAPE '\\'")?;
        let rows = st.query_map(params![like_contains(&email.to_lowercase())], |r| r.get::<_, String>(0))?;
        for j in rows {
            if serde_json::from_str::<Contact>(&j?).is_ok_and(|c| c.emails.iter().any(|e| e.eq_ignore_ascii_case(email))) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ---- kv ----

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .c()
            .query_row("SELECT value FROM kv WHERE key=?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO kv(key,value) VALUES(?1,?2)",
            params![key, value],
        )?;
        Ok(())
    }
}

const MSG_COLS: &str = "id,mailbox_id,folder_id,conversation_id,subject,from_name,from_addr,to_json,received,is_read,is_draft,importance,flag,has_att,categories_json,preview,due_at";

fn message_row(r: &rusqlite::Row) -> rusqlite::Result<MessageSummary> {
    let mut m = message_row_raw(r)?;
    m.recurrence = recurrence_of(&m.categories);
    Ok(m)
}

fn message_row_raw(r: &rusqlite::Row) -> rusqlite::Result<MessageSummary> {
    let from_name: String = r.get(5)?;
    let from_addr: String = r.get(6)?;
    let to_json: String = r.get(7)?;
    let cat_json: String = r.get(14)?;
    Ok(MessageSummary {
        id: r.get(0)?,
        mailbox_id: r.get(1)?,
        folder_id: r.get(2)?,
        conversation_id: r.get(3)?,
        subject: r.get(4)?,
        from: if from_name.is_empty() && from_addr.is_empty() {
            None
        } else {
            Some(Address {
                name: from_name,
                address: from_addr,
            })
        },
        to: serde_json::from_str(&to_json).unwrap_or_default(),
        received_at: r.get(8)?,
        is_read: r.get(9)?,
        is_draft: r.get(10)?,
        importance: r.get(11)?,
        flag: r.get(12)?,
        has_attachments: r.get(13)?,
        categories: serde_json::from_str(&cat_json).unwrap_or_default(),
        preview: r.get(15)?,
        due_at: r.get(16)?,
        recurrence: None,
    })
}

fn mailbox_row(r: &rusqlite::Row) -> rusqlite::Result<Mailbox> {
    Ok(Mailbox {
        id: r.get(0)?,
        account_id: r.get(1)?,
        address: r.get(2)?,
        display_name: r.get(3)?,
        kind: r.get(4)?,
    })
}

fn folder_row(r: &rusqlite::Row) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get(0)?,
        mailbox_id: r.get(1)?,
        parent_id: r.get(2)?,
        display_name: r.get(3)?,
        well_known: r.get(4)?,
        unread_count: r.get(5)?,
        total_count: r.get(6)?,
        child_folder_count: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emails(s: &Store) -> Vec<String> {
        s.list_accounts().unwrap().into_iter().map(|a| a.email).collect()
    }

    #[test]
    fn accounts_keep_the_chosen_order_and_picture() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account("a", "a@x", "A").unwrap();
        s.upsert_account("b", "b@x", "B").unwrap();
        s.upsert_account("c", "c@x", "C").unwrap();
        assert_eq!(emails(&s), ["a@x", "b@x", "c@x"]);
        s.set_account_order(&["c".into(), "a".into(), "b".into()]).unwrap();
        assert_eq!(emails(&s), ["c@x", "a@x", "b@x"]);
        // A new account appears below the arranged ones.
        s.upsert_account("d", "d@x", "D").unwrap();
        assert_eq!(emails(&s), ["c@x", "a@x", "b@x", "d@x"]);
        // Re-login (upsert) keeps order and picture.
        assert!(s.set_account_avatar("a", Some("data:image/png;base64,AAAA")).unwrap());
        s.upsert_account("a", "a@x", "A neu").unwrap();
        let acc = s.list_accounts().unwrap();
        assert_eq!(acc[1].email, "a@x");
        assert_eq!(acc[1].avatar.as_deref(), Some("data:image/png;base64,AAAA"));
        assert!(s.set_account_avatar("a", None).unwrap());
        assert_eq!(s.list_accounts().unwrap()[1].avatar, None);
        assert!(!s.set_account_avatar("nope", None).unwrap());
        // Name and badge survive a re-login as well.
        let badge = AccountBadge { color: Some("#2fa67a".into()), text: Some("CU\nCO".into()) };
        assert!(s.set_account_look("a", Some("Büro"), &badge).unwrap());
        s.upsert_account("a", "a@x", "A").unwrap();
        let acc = s.list_accounts().unwrap();
        assert_eq!(acc[1].label.as_deref(), Some("Büro"));
        assert_eq!(acc[1].badge.as_ref(), Some(&badge));
        assert!(s.set_account_look("a", None, &AccountBadge::default()).unwrap());
        assert_eq!(s.list_accounts().unwrap()[1].badge, None);
    }

    #[test]
    fn old_account_table_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (id TEXT PRIMARY KEY, email TEXT NOT NULL, display_name TEXT NOT NULL,
               created INTEGER NOT NULL DEFAULT (strftime('%s','now')));
             INSERT INTO accounts(id,email,display_name) VALUES('a','a@x','A');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(emails(&s), ["a@x"]);
        s.set_account_order(&["a".into()]).unwrap();
        assert!(s.set_account_avatar("a", Some("data:image/png;base64,AAAA")).unwrap());
        assert!(s.set_account_look("a", Some("A"), &AccountBadge::default()).unwrap());
    }

    fn msg(id: &str, folder: &str, received: &str, read: bool, subject: &str, cats: &[&str]) -> MessageSummary {
        MessageSummary {
            id: id.into(),
            mailbox_id: "mb".into(),
            folder_id: folder.into(),
            conversation_id: "c".into(),
            subject: subject.into(),
            from: Some(Address {
                name: format!("Sender {id}"),
                address: format!("{id}@x"),
            }),
            to: vec![],
            received_at: received.into(),
            is_read: read,
            is_draft: false,
            importance: "normal".into(),
            flag: "notFlagged".into(),
            has_attachments: false,
            categories: cats.iter().map(|s| s.to_string()).collect(),
            preview: String::new(),
            due_at: None,
            recurrence: None,
        }
    }

    fn setup() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account("acc", "a@x", "A").unwrap();
        s.upsert_mailbox(&Mailbox {
            id: "mb".into(),
            account_id: "acc".into(),
            address: "a@x".into(),
            display_name: "A".into(),
            kind: "primary".into(),
        })
        .unwrap();
        s
    }

    #[test]
    fn list_sort_filter() {
        let s = setup();
        s.upsert_messages(&[
            msg("1", "f", "2026-10-01T00:00:00Z", true, "Zebra", &[]),
            msg("2", "f", "2026-10-03T00:00:00Z", false, "Apfel", &["Info"]),
            msg("3", "f", "2026-10-02T00:00:00Z", false, "Mango 50%", &["Info", "Wichtig"]),
            msg("4", "g", "2026-10-04T00:00:00Z", false, "other folder", &[]),
        ])
        .unwrap();
        let mut q = ListQuery {
            mailbox_id: "mb".into(),
            folder_id: "f".into(),
            ..Default::default()
        };
        let p = s.list_messages(&q).unwrap();
        assert_eq!(p.total, 3);
        assert_eq!(p.unread, 2);
        assert_eq!(p.items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["2", "3", "1"]);

        q.sort_by = "subject".into();
        q.sort_desc = false;
        let p = s.list_messages(&q).unwrap();
        assert_eq!(p.items[0].subject, "Apfel");

        q.unread_only = true;
        assert_eq!(s.list_messages(&q).unwrap().total, 2);
        q.unread_only = false;

        q.category = Some("Wichtig".into());
        assert_eq!(s.list_messages(&q).unwrap().items[0].id, "3");
        q.category = None;

        q.text = Some("50%".into());
        assert_eq!(s.list_messages(&q).unwrap().total, 1);
        q.text = Some("sender 1".into());
        assert_eq!(s.list_messages(&q).unwrap().items[0].id, "1");
    }

    #[test]
    fn folders_keep_delta_and_cascade() {
        let s = setup();
        let f = |id: &str, wk: Option<&str>| Folder {
            id: id.into(),
            mailbox_id: "mb".into(),
            parent_id: None,
            display_name: id.into(),
            well_known: wk.map(str::to_string),
            unread_count: 0,
            total_count: 0,
            child_folder_count: 0,
        };
        s.replace_folders("mb", &[f("zz", None), f("in", Some("inbox")), f("dr", Some("drafts"))]).unwrap();
        s.set_delta_link("mb", "in", Some("link")).unwrap();
        s.upsert_messages(&[msg("1", "zz", "2026", true, "x", &[])]).unwrap();
        s.replace_folders("mb", &[f("in", Some("inbox")), f("dr", Some("drafts"))]).unwrap();
        assert_eq!(s.delta_link("mb", "in").unwrap().as_deref(), Some("link"));
        let names: Vec<_> = s.list_folders("mb").unwrap().into_iter().map(|f| f.id).collect();
        assert_eq!(names, ["in", "dr"]);
        assert!(!s.message_exists("mb", "1").unwrap());

        s.delete_account("acc").unwrap();
        assert!(s.list_folders("mb").unwrap().is_empty());
    }

    #[test]
    fn like_wildcards_in_user_input_are_literal() {
        let s = setup();
        s.upsert_messages(&[
            msg("1", "f", "2026-10-01T00:00:00Z", true, "Pfad C:\\temp", &["[K_To_Do]"]),
            msg("2", "f", "2026-10-02T00:00:00Z", true, "Pfad C:temp", &["[K_ToxDo]"]),
            msg("3", "f", "2026-10-03T00:00:00Z", true, "100% sicher", &["a%b"]),
            msg("4", "f", "2026-10-04T00:00:00Z", true, "100 sicher", &["axyzb"]),
        ])
        .unwrap();
        let ids = |q: &ListQuery| s.list_messages(q).unwrap().items.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let mut q = ListQuery {
            mailbox_id: "mb".into(),
            folder_id: "f".into(),
            ..Default::default()
        };
        // "_" and "%" in a category name must not act as wildcards
        q.category = Some("[K_To_Do]".into());
        assert_eq!(ids(&q), ["1"]);
        q.category = Some("a%b".into());
        assert_eq!(ids(&q), ["3"]);
        q.category = None;
        // a backslash in the search text is a plain character
        q.text = Some("C:\\temp".into());
        assert_eq!(ids(&q), ["1"]);
        q.text = Some("100%".into());
        assert_eq!(ids(&q), ["3"]);
        assert_eq!(s.messages_with_category("mb", "[K_To_Do]").unwrap().len(), 1);
    }

    #[test]
    fn contact_lookup_by_email_is_exact() {
        let s = setup();
        let c = |id: &str, mail: &str| Contact {
            id: id.into(),
            account_id: "acc".into(),
            source: "collected".into(),
            display_name: id.into(),
            given_name: String::new(),
            surname: String::new(),
            emails: vec![mail.into()],
            company: String::new(),
            job_title: String::new(),
            phones: vec![],
            notes: String::new(),
            read_only: false,
        };
        // "joanna@x.de" contains "anna@x.de" and is found first
        s.upsert_contact(&c("1", "joanna@x.de")).unwrap();
        s.upsert_contact(&c("2", "anna@x.de")).unwrap();
        assert!(s.contact_with_email("anna@x.de").unwrap());
        assert!(s.contact_with_email("ANNA@x.de").unwrap());
        assert!(!s.contact_with_email("nna@x.de").unwrap());
        // "_" is no wildcard
        assert!(!s.contact_with_email("anna@x_de").unwrap());
        let p = s.list_contacts(Some("x_de"), None, None, 0, 50).unwrap();
        assert_eq!(p.total, 0);
    }
}

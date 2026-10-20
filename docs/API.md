# Frontend ⇄ Backend contract

The UI (`ui/`) never talks to Microsoft Graph directly. It calls the Rust
service layer (`crates/core`, module `service`) through one generic entry point:

| Host | Call | Events |
|------|------|--------|
| Tauri app | `window.__TAURI__.core.invoke('call', { cmd, args })` | `window.__TAURI__.event.listen(name, e => e.payload)` |
| Dev/test server (`crates/devserver`) | `POST /api/call/<cmd>` with JSON body `args` → `200` JSON result, or `4xx/5xx` `{ "error": "message" }` | Server-Sent Events at `GET /api/events`, each `data:` line is `{ "event": name, "payload": {...} }` |

`ui/js/api.js` hides the difference. Errors are human readable German strings.

All JSON keys are **camelCase**. Timestamps are ISO-8601 UTC strings with `Z`
(e.g. `"2026-10-15T14:40:00Z"`) unless noted. Optional values are `null`.

## Shared types

```ts
Address      { name: string, address: string }
Account      { id: string, email: string, displayName: string, kind: "o365"|"imap", mailboxes: Mailbox[], avatar: string|null, label: string|null, badge: AccountBadge|null,
               auth?: "google" /* IMAP account signed in with Google (Gmail / Workspace) */ }
AccountBadge { color: "#rrggbb"|null, text: string|null }   // text: up to 2 lines ("\n") of up to 8 characters
               // kind "imap": no calendar, no mailbox settings (auto reply); categories are local
               // (stored as IMAP keywords on messages), "Wiedervorlage" times are kept locally.
               // auth "google": IMAP/SMTP at imap.gmail.com / smtp.gmail.com with SASL XOAUTH2;
               // "Archiv" is Gmail's "All Mail" (\All), sent copies are filed by Gmail itself.
ServerSettings { host: string, port: number, security: "ssl"|"starttls"|"none", username: string }
ImapAccountInput { email: string, displayName: string, imap: ServerSettings, smtp: ServerSettings,
                   password: string, smtpPassword: string|null /* null = same as IMAP */,
                   auth?: "password"|"google" /* default "password"; "google": passwords unused */ }
Signature    { id: string, name: string, html: string }
SignatureSettings { signatures: Signature[], defaultId: string|null,
                    useOnNew: bool /* default true */, useOnReply: bool /* default true */,
                    useOnForward: bool /* default false */ }
Contact      { id: string, accountId: string, source: "o365"|"local"|"collected",
               displayName: string, givenName: string, surname: string,
               emails: string[], company: string, jobTitle: string,
               phones: { kind: "business"|"mobile"|"home", number: string }[], notes: string,
               readOnly: bool }
ContactInput { displayName, givenName, surname, emails: string[], company, jobTitle,
               phones: {kind, number}[], notes }
Mailbox      { id: string, accountId: string, address: string, displayName: string, kind: "primary" }
Folder       { id: string, mailboxId: string, parentId: string|null, displayName: string,
               wellKnown: "inbox"|"drafts"|"sentitems"|"deleteditems"|"archive"|"junkemail"|"outbox"|"snoozed"|null,
               unreadCount: number, totalCount: number, childFolderCount: number }
MessageSummary {
  id, mailboxId, folderId, conversationId: string,
  subject: string, from: Address|null, to: Address[], receivedAt: string,
  isRead: bool, isDraft: bool, importance: "low"|"normal"|"high",
  flag: "notFlagged"|"flagged"|"complete", hasAttachments: bool,
  categories: string[], preview: string,
  dueAt: string|null,  // follow-up due time (UTC); for snoozed mails = "Wiedervorlage bis"
  recurrence: string|null // "monthly" etc., from the hidden "[#…]" category
}
Attachment   { id, name, contentType: string, size: number, isInline: bool, contentId: string|null }
MessageDetail = MessageSummary & {
  cc: Address[], bcc: Address[], replyTo: Address[], sentAt: string|null,
  bodyHtml: string,            // always HTML; plain text bodies are escaped + <pre>-wrapped;
                               // cid: inline images already replaced by data: URIs
  attachments: Attachment[],   // non-inline only
  internetMessageId: string|null, webLink: string|null,
  isMeetingRequest: bool, eventId: string|null   // set for meeting invitations
}
Category     { id: string, displayName: string, color: string /* "preset0".."preset24" or "none" */ }
Calendar     { id, accountId, name: string, color: string|null /* hex like "#4a90d9" */, canEdit: bool, isDefault: bool }
Attendee     { name, address, type: "required"|"optional"|"resource",
               response: "none"|"organizer"|"tentativelyAccepted"|"accepted"|"declined"|"notResponded" }
Recurrence   { pattern: "daily"|"weekly"|"absoluteMonthly"|"absoluteYearly", interval: number,
               daysOfWeek: string[] /* "monday".. */, dayOfMonth: number, month: number,
               startDate: "YYYY-MM-DD", endDate: "YYYY-MM-DD"|null, occurrences: number|null }
Event {
  id, accountId, calendarId: string, subject: string,
  start: string, end: string,          // UTC ISO with Z. All-day: midnight local day boundaries converted to UTC as returned by Graph
  isAllDay: bool, location: string, organizer: Address|null, attendees: Attendee[],
  showAs: "free"|"tentative"|"busy"|"oof"|"workingElsewhere"|"unknown",
  categories: string[], isOrganizer: bool,
  responseStatus: "none"|"organizer"|"tentativelyAccepted"|"accepted"|"declined"|"notResponded",
  bodyPreview: string, isOnlineMeeting: bool, onlineMeetingUrl: string|null,
  recurrence: Recurrence|null, seriesMasterId: string|null,
  type: "singleInstance"|"occurrence"|"exception"|"seriesMaster",
  isReminderOn: bool, reminderMinutes: number,
  importance: "low"|"normal"|"high", sensitivity: "normal"|"personal"|"private"|"confidential",
  isCancelled: bool, webLink: string|null
}
EventDetail = Event & { bodyHtml: string }
EventInput {
  subject, start: "YYYY-MM-DDTHH:MM:SS" /* local wall time, no Z */, end, timeZone: string /* IANA, e.g. "Europe/Berlin" */,
  isAllDay: bool, location: string, bodyHtml: string,
  attendees: { name, address, type }[], showAs, categories: string[],
  isReminderOn: bool, reminderMinutes: number, isOnlineMeeting: bool,
  recurrence: Recurrence|null, sensitivity, importance
}
Draft {
  id: string|null, mailboxId: string, mode: "new"|"reply"|"replyAll"|"forward",
  to: Address[], cc: Address[], bcc: Address[], subject: string, bodyHtml: string,
  importance: "low"|"normal"|"high", requestReadReceipt: bool, attachments: Attachment[]
}
DraftInput = Draft (without attachments) & {
  newAttachments: { name, contentType, contentBase64 }[], removedAttachmentIds: string[],
  remindAt?: string|null   // "Erinnerung an mich": ISO UTC. After sending, a reminder mail
                           // ("Nachfassen: <subject>", to = original recipients, replyTo = them,
                           // body = sent text) is created directly in "Wiedervorlage", due at remindAt.
}
Config {
  clientId: string, tenant: string /* empty → built-in default "organizations" (multi-tenant) */, syncIntervalSecs: number,
  loadRemoteImages: bool, readingPane: "right"|"bottom"|"off",
  markReadDelaySecs: number /* -1 = never automatically */, listMode: "comfortable"|"compact",
  showDateSeparators: bool /* thin "Heute / Gestern / Älter" separators in the list */,
  snoozeOptions: SnoozeOption[] /* entries of the "Wiedervorlage" dropdown, editable in settings */,
  demo: bool /* read-only: running against the built-in demo server */,
  builtinClientId: bool /* read-only: a client ID is compiled in → welcome dialog only needs "Anmelden" */,
  googleClientId: string, googleClientSecret: string /* OAuth client ("Desktop app") for Gmail; empty → built-in, if any */,
  builtinGoogleClient: bool /* read-only: a Google client is compiled in */
}
SnoozeOption { label: string, rule: string }
// Kanban lanes are categories named "[K_<lane>]" (LANE_PREFIX "[K_", suffix "]").
// The UI hides them everywhere except the Kanban board. Lane names: letters, digits,
// space, "-" and "_", 1–30 chars, unique per account.
// Config also has: kanbanReadingPane: "bottom"|"right" (default "bottom").
// rule grammar (evaluated in the UI, local time):  "<base>@HH:MM"
//   base = "tomorrow" | "days:N" (today + N days) | "weekend" (next Saturday) | "nextWeek" (next Monday)
// defaults: Morgen 8 Uhr = tomorrow@08:00, Übermorgen 8 Uhr = days:2@08:00, Am Wochenende = weekend@08:00,
//           Nächste Woche = nextWeek@08:00, In 14 Tagen = days:14@08:00, In 30 Tagen = days:30@08:00
AutoReply { status: "disabled"|"alwaysEnabled"|"scheduled", internalReplyMessage: string,
            externalReplyMessage: string, scheduledStart: string|null, scheduledEnd: string|null,
            externalAudience: "none"|"contactsOnly"|"all" }
```

## Commands (`cmd` → args → result)

### App / accounts
| cmd | args | result |
|-----|------|--------|
| `app_info` | `{}` | `{ version: string }` |
| `get_config` | `{}` | `Config` |
| `save_config` | `{ config: Config }` | `Config` |
| `list_accounts` | `{}` | `Account[]` |
| `add_account` | `{}` | `Account` — Microsoft 365: opens system browser, waits for login (up to 5 min) |
| `discover_imap` | `{ email }` | `{ imap: ServerSettings, smtp: ServerSettings, source: "ispdb"\|"autoconfig"\|"guess" }` — Thunderbird ISPDB / autoconfig lookup, else guesses (imap./smtp.<domain>) |
| `test_imap_account` | `{ input: ImapAccountInput }` | `{ imapOk: bool, smtpOk: bool, imapError: string\|null, smtpError: string\|null }` |
| `add_imap_account` | `{ input: ImapAccountInput }` | `Account` — verifies IMAP+SMTP login first, password goes to the OS keychain |
| `add_google_account` | `{ loginHint: string\|null }` | `Account` — "Mit Google anmelden": opens the system browser (Google OAuth, PKCE, loopback redirect), waits up to 5 min, verifies IMAP+SMTP via XOAUTH2; the refresh token goes to the OS keychain. Signing in to an existing account renews its login. |
| `update_imap_account` | `{ accountId, input: ImapAccountInput }` | `Account` (password "" = keep the stored one; Google accounts: only `displayName` changes) |
| `get_imap_account` | `{ accountId }` | `ImapAccountInput` with password "" |
| `remove_account` | `{ accountId }` | `null` |
| `set_account_order` | `{ accountIds: string[] }` | `null` — display order from top to bottom (folder pane, menus, „Von“); emits `accounts-changed` |
| `set_account_avatar` | `{ accountId, avatar: string\|null }` | `null` — own picture as `data:image/png\|jpeg\|webp;base64,…` (max. 300 KB; the UI crops it to 128 px), `null` = initials again; emits `accounts-changed` |
| `set_account_look` | `{ accountId, label: string\|null, badge: { color, text }\|null }` | `null` — name in the views instead of the address (max. 60 characters) and the round badge without a picture: colour `#rrggbb` and up to 2 lines of up to 8 characters; empty/`null` = address, derived colour, initials; emits `accounts-changed` |
| `set_active_folder` | `{ mailboxId, folderId }` | `null` — background sync also syncs this folder |
| `sync_all` | `{}` | `null` — "Senden/Empfangen": folders + inbox + active folder of every mailbox |

### Folders
| `list_folders` | `{ mailboxId }` | `Folder[]` (from cache; flat list, build tree via parentId; sorted: well-known first in Outlook order, then alphabetical) |
| `sync_folders` | `{ mailboxId }` | `Folder[]` |
| `create_folder` | `{ mailboxId, parentId: string\|null, name }` | `Folder` |
| `rename_folder` | `{ mailboxId, folderId, name }` | `Folder` |
| `delete_folder` | `{ mailboxId, folderId }` | `null` |
| `mark_folder_read` | `{ mailboxId, folderId }` | `null` |
| `empty_folder` | `{ mailboxId, folderId }` | `null` (permanently deletes all items; used for "Gelöschte Elemente leeren") |

### Messages
| `list_messages` | `{ mailboxId, folderId, sortBy: "received"\|"from"\|"subject"\|"importance"\|"flag"\|"attachments"\|"categories", sortDesc: bool, unreadOnly: bool, flaggedOnly: bool, category: string\|null, text: string\|null, offset: number, limit: number }` | `{ items: MessageSummary[], total: number, unread: number, canLoadOlder: bool }` — from local cache; `text` filters subject/from/preview locally |
| `sync_folder` | `{ mailboxId, folderId }` | `{ added, updated, removed }` |
| `load_older` | `{ mailboxId, folderId }` | `number` loaded (0 = nothing older) |
| `search_messages` | `{ mailboxId, text, folderId: string\|null, field?: "all"\|"from"\|"to"\|"subject"\|"body", hasAttachments?: bool }` | `MessageSummary[]` (server side search, max 100; folderId null = all folders) |
| `archive_messages` | `{ mailboxId, ids }` | `null` — move to the Archive folder (created if missing) |
| `snooze_messages` | `{ mailboxId, ids, until: ISO UTC }` | `null` — "Auf Wiedervorlage": moves to folder "Wiedervorlage" (Folder.wellKnown = "snoozed"), stores the time as follow-up due date; the background sync moves it back to the inbox (unread) when due and emits `new-mail` |
| `unsnooze_messages` | `{ mailboxId, ids }` | `null` — back to the inbox now |
| `get_message` | `{ mailboxId, messageId }` | `MessageDetail` |
| `set_read` | `{ mailboxId, ids: string[], isRead: bool }` | `null` |
| `set_flag` | `{ mailboxId, ids, flag }` | `null` |
| `set_categories` | `{ mailboxId, ids, categories: string[] }` | `null` (replaces the list) |
| `move_messages` | `{ mailboxId, ids, destFolderId }` | `null` |
| `copy_messages` | `{ mailboxId, ids, destFolderId }` | `null` |
| `create_task` | `{ mailboxId, subject, bodyHtml, due: ISO\|null }` | `MessageSummary` — "Aufgabe (Mail)": a normal (non-draft) mail from yourself to yourself, created directly on the server: due null = "Sofort" → Posteingang (unread); otherwise → "Wiedervorlage" with dueAt |
| `delete_messages` | `{ mailboxId, ids, permanent: bool }` | `null` (non-permanent = move to Deleted Items; items already in Deleted Items are removed permanently) |
| `download_attachment` | `{ mailboxId, messageId, attachmentId, action: "open"\|"save" }` | `string\|null` saved path (null if save dialog cancelled) |
| `get_message_source` | `{ mailboxId, messageId }` | `string` (MIME source, for "Quelltext anzeigen") |

### Compose
| `create_draft` | `{ mailboxId, mode, sourceMessageId: string\|null }` | `Draft` (`new` → id null, body = signature; reply/replyAll/forward → server draft with quoted original + signature on top) |
| `get_draft` | `{ mailboxId, messageId }` | `Draft` (open an existing draft from the Drafts folder) |
| `save_draft` | `{ draft: DraftInput }` | `Draft` (creates server draft when id is null) |
| `send_draft` | `{ draft: DraftInput }` | `null` |
| `discard_draft` | `{ mailboxId, draftId: string\|null }` | `null` |
| `suggest_recipients` | `{ mailboxId, query }` | `Address[]` (max 10; local contacts first, then O365 People API, then known senders) |

`create_draft` inserts the account's default signature according to SignatureSettings
(`useOnNew` / `useOnReply` for reply+replyAll / `useOnForward`). The signature is wrapped in
`<div class="signature" data-signature-id="…">…</div>` so the compose window can swap it.

### Inbox rules (automatic, before display)
New mails arriving in the inbox pass a rule chain during sync **before** `mail-changed` is emitted;
matched mails are moved server-side and never show up in the inbox list. Rule 1: mails from
yourself to yourself whose subject contains a due token are snoozed accordingly:
`#heute` (today 17:00, or +1h if later), `#morgen`, `#übermorgen`, `#wochenende`, `#nächstewoche`,
`#<N>t` / `#<N>d` (in N days), `#<N>w` (in N weeks) — times per the matching snooze option or 08:00.
Event `rules-applied` `{ mailboxId, count, subjects: string[] }` after mails were moved.

### Kanban lanes (per account)
| `get_lanes` | `{ accountId }` | `string[]` lane names in order |
| `save_lanes` | `{ accountId, lanes: string[] }` | `string[]` — validates names, creates missing "[K_…]" categories (O365 master categories / IMAP local), deletes categories of removed lanes and strips them from cached messages; renaming = remove + add |
| `set_lane` | `{ mailboxId, ids, lane: string\|null }` | `null` — replaces any "[K_…]" category on the messages, keeps all other categories; null = no lane |

### Follow-ups resolve themselves ("Nachfassen erledigt sich selbst")
Reminder mails from `remindAt` carry a hidden category `"[F_<10 hex>]"` (hash of the sent mail's
thread id). When a reply from someone else arrives in that thread, the inbox rule chain archives the
reminder (wherever it is: Wiedervorlage or inbox) and emits `followup-resolved`
`{ mailboxId, subject, from }`. "[F_…]" categories are hidden in the UI like lane categories.

### Recurring tasks ("Wiederkehrend")
Hidden category on the mail: `"[#daily]"`, `"[#weekdays]"`, `"[#weekly]"`, `"[#biweekly]"`, `"[#monthly]"`,
`"[#quarterly]"`, `"[#yearly]"`. Fixed rhythm: when such a mail is **archived**, a copy (same subject,
body, categories) is placed in "Wiedervorlage" due at the next occurrence after its due time (missed
occurrences are skipped; without due time the anchor is "now"); the event `recurrence-scheduled`
`{ mailboxId, subject, due }` follows. Recurring mails keep their dueAt when they return to the inbox.
| `create_task` | gains optional `recurrence: "daily"\|"weekdays"\|"weekly"\|"biweekly"\|"monthly"\|"quarterly"\|"yearly"\|null` |
| `set_recurrence` | `{ mailboxId, ids, recurrence: string\|null }` | `null` |
MessageSummary gains `recurrence: string|null` (derived from the hidden category).

### Heute
| `list_today` | `{}` | `{ due: MessageSummary[], todo: { accountId, lane: string, items: MessageSummary[] }[] }` — all mails of all accounts whose dueAt is before the end of today (local time; snoozed and returned ones), plus per account the mails in its FIRST Kanban lane |

### Fokuszeit
| `get_focus` | `{}` | `{ until: ISO\|null }` |
| `set_focus` | `{ minutes: number }` | `{ until: ISO\|null }` — 0 ends focus time. While active the background sync is paused (no auto fetch, no notifications, due snoozes are not returned); `sync_all` (manual "Senden/Empfangen") still works. Event `focus-changed` `{ until }`; when it expires the backend emits `focus-changed` `{ until: null }` and syncs once. |

### Signatures (per account, stored locally)
| `get_signatures` | `{ accountId }` | `SignatureSettings` |
| `save_signatures` | `{ accountId, settings: SignatureSettings }` | `SignatureSettings` (ids are generated for new entries with id "") |

### Contacts (local cache; O365 contacts synced via delta, sent-to addresses collected automatically)
| `list_contacts` | `{ query: string\|null, accountId: string\|null, offset, limit }` | `{ items: Contact[], total }` sorted by displayName |
| `get_contact` | `{ id }` | `Contact` |
| `sync_contacts` | `{ accountId }` | `{ added, updated, removed }` (also part of background sync) |
| `create_contact` | `{ accountId, input: ContactInput }` | `Contact` (O365 account → created in Outlook; IMAP → local) |
| `update_contact` | `{ id, input: ContactInput }` | `Contact` (collected contacts become local ones) |
| `delete_contact` | `{ id }` | `null` |

### Categories (per account – Outlook master category list)
| `list_categories` | `{ accountId }` | `Category[]` |
| `create_category` | `{ accountId, displayName, color }` | `Category` |
| `update_category` | `{ accountId, id, color }` | `Category` (Graph cannot rename) |
| `delete_category` | `{ accountId, id }` | `null` |

### Calendar
| `list_calendars` | `{ accountId }` | `Calendar[]` |
| `list_events` | `{ accountId, calendarIds: string[], start: ISO, end: ISO }` | `Event[]` (expanded occurrences in range) |
| `get_event` | `{ accountId, eventId }` | `EventDetail` |
| `create_event` | `{ accountId, calendarId, input: EventInput }` | `Event` |
| `update_event` | `{ accountId, eventId, input: EventInput }` | `Event` |
| `delete_event` | `{ accountId, eventId }` | `null` (organizer with attendees: sends cancellation) |
| `respond_event` | `{ accountId, eventId, response: "accept"\|"tentativelyAccept"\|"decline", comment: string, sendResponse: bool }` | `null` |

### Mailbox settings
| `get_auto_reply` | `{ accountId }` | `AutoReply` |
| `set_auto_reply` | `{ accountId, autoReply: AutoReply }` | `AutoReply` |

### Host
| `open_window` | `{ kind: "compose"\|"message"\|"event", params: object }` | `null` — opens `ui/<kind>.html?<params as query string>` in a new window |
| `open_url` | `{ url }` | `null` — opens in the system browser |

## Events (backend → UI)
| name | payload |
|------|---------|
| `sync-status` | `{ state: "syncing"\|"idle"\|"offline"\|"error", message: string, lastSync: string\|null }` |
| `mail-changed` | `{ mailboxId: string, folderId: string\|null }` — reload folder counts / list |
| `calendar-changed` | `{ accountId: string }` |
| `accounts-changed` | `{}` |
| `categories-changed` | `{ accountId: string }` |
| `contacts-changed` | `{}` |
| `new-mail` | `{ mailboxId, count, from: string, subject: string }` |

Windows opened with `open_window` are separate pages that use the same API; changes they make are
broadcast through the events above so the main window refreshes.

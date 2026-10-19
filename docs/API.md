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
Account      { id: string, email: string, displayName: string, mailboxes: Mailbox[] }
Mailbox      { id: string, accountId: string, address: string, displayName: string, kind: "primary" | "shared" }
Folder       { id: string, mailboxId: string, parentId: string|null, displayName: string,
               wellKnown: "inbox"|"drafts"|"sentitems"|"deleteditems"|"archive"|"junkemail"|"outbox"|null,
               unreadCount: number, totalCount: number, childFolderCount: number }
MessageSummary {
  id, mailboxId, folderId, conversationId: string,
  subject: string, from: Address|null, to: Address[], receivedAt: string,
  isRead: bool, isDraft: bool, importance: "low"|"normal"|"high",
  flag: "notFlagged"|"flagged"|"complete", hasAttachments: bool,
  categories: string[], preview: string
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
  newAttachments: { name, contentType, contentBase64 }[], removedAttachmentIds: string[]
}
Config {
  clientId: string, tenant: string /* "organizations" default */, syncIntervalSecs: number,
  loadRemoteImages: bool, signatureHtml: string, readingPane: "right"|"bottom"|"off",
  markReadDelaySecs: number /* -1 = never automatically */, listMode: "columns"|"compact",
  demo: bool /* read-only: running against the built-in demo server */
}
AutoReply { status: "disabled"|"alwaysEnabled"|"scheduled", internalReplyMessage: string,
            externalReplyMessage: string, scheduledStart: string|null, scheduledEnd: string|null,
            externalAudience: "none"|"contactsOnly"|"all" }
```

## Commands (`cmd` → args → result)

### App / accounts
| cmd | args | result |
|-----|------|--------|
| `get_config` | `{}` | `Config` |
| `save_config` | `{ config: Config }` | `Config` |
| `list_accounts` | `{}` | `Account[]` |
| `add_account` | `{}` | `Account` — opens system browser, waits for login (up to 5 min) |
| `remove_account` | `{ accountId }` | `null` |
| `add_shared_mailbox` | `{ accountId, address }` | `Mailbox` |
| `remove_mailbox` | `{ mailboxId }` | `null` |
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
| `search_messages` | `{ mailboxId, text, folderId: string\|null }` | `MessageSummary[]` (server side search, max 100) |
| `get_message` | `{ mailboxId, messageId }` | `MessageDetail` |
| `set_read` | `{ mailboxId, ids: string[], isRead: bool }` | `null` |
| `set_flag` | `{ mailboxId, ids, flag }` | `null` |
| `set_categories` | `{ mailboxId, ids, categories: string[] }` | `null` (replaces the list) |
| `move_messages` | `{ mailboxId, ids, destFolderId }` | `null` |
| `copy_messages` | `{ mailboxId, ids, destFolderId }` | `null` |
| `delete_messages` | `{ mailboxId, ids, permanent: bool }` | `null` (non-permanent = move to Deleted Items; items already in Deleted Items are removed permanently) |
| `download_attachment` | `{ mailboxId, messageId, attachmentId, action: "open"\|"save" }` | `string\|null` saved path (null if save dialog cancelled) |
| `get_message_source` | `{ mailboxId, messageId }` | `string` (MIME source, for "Quelltext anzeigen") |

### Compose
| `create_draft` | `{ mailboxId, mode, sourceMessageId: string\|null }` | `Draft` (`new` → id null, body = signature; reply/replyAll/forward → server draft with quoted original + signature on top) |
| `get_draft` | `{ mailboxId, messageId }` | `Draft` (open an existing draft from the Drafts folder) |
| `save_draft` | `{ draft: DraftInput }` | `Draft` (creates server draft when id is null) |
| `send_draft` | `{ draft: DraftInput }` | `null` |
| `discard_draft` | `{ mailboxId, draftId: string\|null }` | `null` |
| `suggest_recipients` | `{ mailboxId, query }` | `Address[]` (max 10) |

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
| `new-mail` | `{ mailboxId, count, from: string, subject: string }` |

Windows opened with `open_window` are separate pages that use the same API; changes they make are
broadcast through the events above so the main window refreshes.

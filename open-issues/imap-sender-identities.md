# Multiple sender identities per IMAP/SMTP account

Status: open (feedback, not yet scheduled)

## Goal

Support several sender identities per IMAP/SMTP account. The **account** holds
connection settings and credentials. A **sender identity** holds an email
address, a display name and optionally a Reply-To address and a signature.
Several identities can share one account. The login name does not have to be an
email address, and IMAP and SMTP credentials may differ.

## Requirements

### Capturing identities
- IMAP has no standard command to list alias addresses, so identities are added,
  edited and imported manually.
- Existing account configurations or explicitly supported provider APIs may be
  additional sources.
- Addresses seen in messages may be offered as *suggestions* only; they never
  count as allowed senders automatically.

### Keeping the account assignment
- Every identity belongs to exactly one account.
- Sending uses that account's configured SMTP connection.
- Additional identities never create separate IMAP connections or mailboxes.

### Choosing the sender
- The composer offers a clear "From" selector.
- Address, display name, signature and (if set) Reply-To are taken from the
  chosen identity.
- One default identity per account.
- A manual choice always wins over automatic suggestions.

### Picking the sender for replies
- Compare the original message's recipients with the identities of the
  account it belongs to: exact match in To first, then Cc.
- Further delivery information (e.g. `Delivered-To`) only if available and
  trustworthy.
- For messages sent by the user, keep that message's own sender identity.
- Without a unique match: use the default or let the user choose.
- Same domain alone is **not** a match.
- Plus addressing and other provider-specific rules only when explicitly
  configured.

### Sending correctly
- Keep SMTP authentication, the visible `From` header and the SMTP envelope
  sender (`MAIL FROM`) separate.
- `From` is the chosen identity.
- The envelope sender is the configured address the provider allows; for normal
  aliases usually the same address.
- Never silently replace the chosen sender with the login name.
- Reply-To is not a substitute for From.

### Provider restrictions
- A locally created identity does not grant server-side permission to send.
- Show rejections in plain language and keep the draft.
- Never retry silently with a different sender.

### Persisting the choice
- Store the identity with the draft and restore it when the draft is reopened.
- If the identity was removed in the meantime, require a valid choice.
- File sent messages according to the account configuration and avoid a second
  copy when the server already stores one.

## Testing (optional)
Multiple accounts, several identities per account, different IMAP/SMTP login
names, replies to aliases, reopened drafts and rejected sender addresses.
No test mails may be sent to real recipients automatically.

## Starting points in the code
- `crates/core/src/service/imap_account.rs` – account model (`username` per server)
- `crates/core/src/imap/smtp.rs` – `send(settings, credential, envelope, raw)`; envelope already separate
- `crates/core/src/service/imap_compose.rs` – building and sending IMAP drafts
- `crates/core/src/service/signatures.rs`, `ui/js/compose.js` – signatures / composer

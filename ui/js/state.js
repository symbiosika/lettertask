// Shared app state for the main window.
import { call } from './api.js';
import { setCategories } from './categories.js';

export const state = {
  config: null,
  accounts: [],
  folders: new Map(), // mailboxId -> Folder[]
  current: { mailboxId: null, folderId: null },
  mode: 'mail',
  sync: { state: 'idle', message: '', lastSync: null },
};

export function mailboxes() {
  return state.accounts.flatMap((a) => a.mailboxes || []);
}
export function mailboxById(id) { return mailboxes().find((m) => m.id === id) || null; }
export function accountOfMailbox(mailboxId) {
  const mb = mailboxById(mailboxId);
  return mb ? mb.accountId : (state.accounts[0] && state.accounts[0].id) || null;
}
export function foldersOf(mailboxId) { return state.folders.get(mailboxId) || []; }
export function folderById(mailboxId, folderId) { return foldersOf(mailboxId).find((f) => f.id === folderId) || null; }
export function wellKnown(mailboxId, name) { return foldersOf(mailboxId).find((f) => f.wellKnown === name) || null; }

export async function loadConfig() {
  state.config = await call('get_config', {}, { silent: true });
  return state.config;
}
export async function loadAccounts() {
  state.accounts = (await call('list_accounts', {}, { silent: true })) || [];
  return state.accounts;
}
export async function loadFolders(mailboxId) {
  const list = (await call('list_folders', { mailboxId }, { silent: true })) || [];
  state.folders.set(mailboxId, list);
  return list;
}
export async function loadCategories(accountId) {
  const list = (await call('list_categories', { accountId }, { silent: true })) || [];
  setCategories(accountId, list);
  return list;
}

/** Standalone windows: loads accounts + categories so shared helpers work. */
export async function bootstrapSecondary() {
  await Promise.all([loadConfig(), loadAccounts()]);
  await Promise.all(state.accounts.map((a) => loadCategories(a.id).catch(() => [])));
}

# Histórico de notificações com dono único — plano

**Bug:** cada applet Central (um por monitor) escuta o session bus sozinho, guarda o histórico na
memória, lê `notifications.json` só ao iniciar e regrava a lista INTEIRA a cada evento. Resultado:
quem grava por último ganha. Remover um item `restored` ou usar o Limpar só vale para o applet local,
e os outros ressuscitam o item na próxima gravação. O `seen` (contador de novas) também fica separado
por applet, e o applet que reiniciou marca como `restored` (sem ações) itens que os outros mostram vivos.

**Correção:** igual ao clipboard. Um processo `cosmic-ext-applet-central --notifications`
(systemd --user + ativação D-Bus) é o ÚNICO que escuta o bus e grava
`~/.local/state/io.github.Aenes0.CosmicCentral/notifications.json` (tmp+rename, mutex). A cada mudança
ele emite `Changed()`. Os applets só leem o arquivo (no início e a cada `Changed`) e pedem mudanças
por método D-Bus. Sem timer e sem polling: o repouso não acorda nada.
Os applets deixam de ser monitores do bus (eram 3 BecomeMonitor) e de varrer os `.desktop` (eram 3 caches).

## Contrato (todas as tasks seguem isto)

Módulo `src/notifications/` (substitui `src/notifications.rs`; `git mv` não é necessário, só mover o conteúdo):

### `src/notifications/mod.rs` — tipo, regras puras, Store, lado do applet
```rust
pub mod service;      // processo --notifications
mod watch;            // escuta do bus (só o serviço usa)

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Notification {
    pub id: u32, pub app: String, pub icon: String, pub summary: String, pub body: String,
    pub actions: Vec<(String, String)>, pub time: std::time::SystemTime,
    #[serde(default)] pub restored: bool,
    /// Shown in a popup already: not counted as new. Shared by all applets.
    #[serde(default)] pub seen: bool,
}
/// Kept at most, newest first.
pub const KEEP: usize = 100;

#[derive(Clone, Debug)]
pub enum Event { New(Notification), Closed(u32), Dismissed(u32) }   // same meaning as today

/// Pure rules (unit tested):
pub fn apply(list: &mut Vec<Notification>, event: Event);
//  New(n): retain(|o| o.restored || o.id != n.id); insert(0, n); truncate(KEEP)
//  Closed(id): retain(|o| o.restored || o.id != id)
//  Dismissed(id): the live one with that id → restored = true, actions.clear()
pub fn remove(list: &mut Vec<Notification>, id: u32, restored: bool); // retain (id,restored) != key
pub fn clear(list: &mut Vec<Notification>) -> Vec<u32>;             // drains all, returns live (not restored) ids
pub fn mark_seen(list: &mut Vec<Notification>);                     // seen = true on all
pub fn revive(list: &mut Vec<Notification>);  // on service start: restored = true, actions.clear(), seen = true
                                              // (icon/body re-resolution lives in watch::tidy, called by the service)

pub struct Store { dir: PathBuf }
impl Store {
    pub fn user() -> Store;            // $XDG_STATE_HOME or ~/.local/state, joined with crate::APP_ID
    #[cfg(test)] pub fn at(dir: PathBuf) -> Store;
    pub fn load(&self) -> Vec<Notification>;               // missing/invalid → empty
    pub fn change(&self, f: impl FnOnce(&mut Vec<Notification>)) -> Vec<Notification>;
    // static Mutex held read→write; reads the file as it is now; invalid file renamed to
    // notifications.json.bad (never overwritten unseen); writes notifications.json.tmp then rename.
}
pub fn load() -> Vec<Notification> { Store::user().load() }

// Applet side (same shape as src/clipboard/panel.rs):
pub const NAME: &str = "io.github.Aenes0.CosmicCentral.Notifications";
pub const PATH: &str = "/io/github/Aenes0/CosmicCentral/Notifications";
#[zbus::proxy(interface = "io.github.Aenes0.CosmicCentral.Notifications",
              default_service = "io.github.Aenes0.CosmicCentral.Notifications",
              default_path = "/io/github/Aenes0/CosmicCentral/Notifications")]
trait History { fn remove(&self, id: u32, restored: bool) -> zbus::Result<()>; fn clear(&self) -> zbus::Result<()>;
                fn seen(&self) -> zbus::Result<()>; #[zbus(signal)] fn changed(&self) -> zbus::Result<()>; }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Action { Remove(u32, bool), Clear, Seen }
pub async fn request(conn: &zbus::Connection, action: Action) -> zbus::Result<()>;
pub async fn ask(action: Action);                         // OnceCell connection, 30 s method timeout, logs failure
pub fn subscription() -> cosmic::iced::Subscription<()>;  // one () on listen + one per Changed; reconnect after 5 s
pub async fn invoke(id: u32, action: String);             // UNCHANGED from today (daemon's InvokeAction)
pub async fn dismiss(ids: Vec<u32>);                      // UNCHANGED from today (CloseNotification)
```

### `src/notifications/watch.rs` — moved verbatim from today's notifications.rs
`Signal`, `skip_in_history`, `bus_notifications`, `event` (with `opened`), `app_icon`, `desktop_icon`,
`plain`, the two daemon proxies, `DISMISSED_BY_USER`, and its tests (markup, popups_closed,
history_skips_transient_and_osd, sent_icon). New:
```rust
pub async fn events() -> zbus::Result<impl futures::Stream<Item = super::Event>>;
// what today's subscription() builds: monitor stream + NotificationClosed + ActionInvoked, select_all,
// mapped through event(&mut opened, ..). Errors setting up are returned, not swallowed.
pub fn tidy(list: &mut Vec<super::Notification>); // icon = app_icon(app, None, icon, None); body = plain(body)
```
(`invoke`/`dismiss` stay in mod.rs and use the daemon proxies, so they need `pub(super)` access or live in mod.rs.)

### `src/notifications/service.rs` — the process
```rust
pub struct Service { store: Arc<Store> }
#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Notifications")]
impl Service {
    async fn remove(&self, id: u32, restored: bool, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()>;
    async fn clear(&self, e) -> zbus::fdo::Result<()>;   // history only; the applet closes the live ones on the daemon
    async fn seen(&self, e) -> zbus::fdo::Result<()>;
    #[zbus(signal)] async fn changed(e: &SignalEmitter<'_>) -> zbus::Result<()>;
}
pub async fn serve(builder: zbus::connection::Builder<'_>, service: Service) -> zbus::Result<zbus::Connection>;
// name(NAME), replace_existing_names(false), serve_at(PATH)
pub fn main() -> !;
// current_thread tokio runtime; serve (NameTaken → log + exit 1); store.change(revive + watch::tidy);
// emit Changed; then select!: watch::events() → store.change(apply) → Changed  |  bus closed → exit 1.
// If watch::events() fails to start: log error, exit 1 (systemd Restart=on-failure).
```
`src/main.rs`: `if args has "--notifications" { notifications::service::main() }` next to `--clipboard`.

### Applet wiring (`src/app.rs`, pointwise edits; DO NOT touch clipboard lines)
- `Message::Notification(notifications::Event)` → `Message::NotificationsChanged`; handler: `self.notifications = notifications::load()`.
- subscription: `notifications::subscription().map(|()| Message::NotificationsChanged)`.
- `remove_notification(key)`: local `notifications::remove` (instant UI) + return/batch a Task that does
  `notifications::ask(Action::Remove(id, restored))`. No more `notifications::save`.
- `ClearNotifications`: `let ids = notifications::clear(&mut self.notifications)`; ask `Action::Clear`, then `dismiss(ids)`.
- Field `seen: HashSet<u32>` goes away; `is_new(n) = !n.restored && !n.seen`; `mark_seen()` sets local
  `notifications::mark_seen` and the caller returns a Task asking `Action::Seen` (only if something was unseen).
  `self.media_outputs = false` stays in mark_seen.
- Open/Dismiss/Action keep calling `invoke`/`dismiss` exactly as today.

### Units (new files; install.sh lines handed to the parent session)
`data/central-notifications.service.in`, `data/io.github.Aenes0.CosmicCentral.Notifications.service.in`
— copies of the clipboard ones with the name/flag changed.

### Tests
- `apply`/`remove`/`clear`/`mark_seen`/`revive`: unit tests, including adversarial ones (a replaced id
  keeps one entry; a restored one with the same id as a new one survives; Closed never removes a restored one;
  KEEP truncation; Dismissed of an unknown id changes nothing).
- Store: tmp+rename, invalid file → .bad, two threads changing don't lose writes, two `Store`s on the same dir see each other.
- Service over a private dbus-daemon (config WITHOUT standard service dirs, so nothing installed gets activated):
  Remove/Clear/Seen change the file and emit Changed; a second `serve` gets NameTaken; `request` without the service → Err.
- "Same history on every applet": two independent loads after a service change are equal.

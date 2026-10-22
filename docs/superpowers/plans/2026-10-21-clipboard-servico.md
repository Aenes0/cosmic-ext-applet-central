# Histórico da área de transferência num processo próprio — Plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Um serviço `cosmic-ext-applet-central --clipboard` (systemd `--user`) dono único da conexão `ext-data-control` e do `clipboard.json`, que mantém a última cópia colável depois que o app de origem fecha; os applets só leem o arquivo e pedem ações por D-Bus.

**Architecture:** `src/clipboard.rs` vira o módulo `src/clipboard/`: `mod.rs` (histórico em disco, `Store`), `keep.rs` (lógica pura do "manter colável" + leitor de pipes com `poll`), `service.rs` (loop calloop/Wayland + servidor zbus numa thread tokio `current_thread`), `panel.rs` (UI atual + cliente D-Bus). Nenhuma dependência nova.

**Tech Stack:** Rust 2024 (rustc 1.99), libcosmic/iced, `cosmic::cctk` (wayland-client 0.31, calloop 0.14, protocolo `ext_data_control_v1`), zbus 5.17 (feature `tokio`), tokio, libc, image.

**Spec:** `docs/superpowers/specs/2026-10-21-clipboard-servico-design.md`

## Global Constraints

- Shell num sandbox Flatpak: `cargo`/`git` só via `flatpak-spawn --host sh -c 'cd ~/Projetos/quick-settings && …'`.
- **Não commitar** (nenhum `git commit/stash/reset/push`). Os passos "Commit" do template **não existem** neste plano.
- **Não instalar** (`./install.sh`, `.dev/recarregar.sh`, `systemctl --user enable/restart`): recarrega o painel; só o controller, com o OK do usuário (Task 6).
- Working tree compartilhado com outra frente (`src/tray/`, `--tray` em `main.rs`, `install.sh`, `Cargo.toml`, `README.md`, `show_desktop.rs`): em `main.rs` e `install.sh` só edições pontuais com Edit, relendo antes; não tocar nos outros arquivos dessa frente.
- Valores: `KEEP = 15` (além dos fixados), `PREVIEW_CHARS = 200`, `THUMB_HEIGHT = 96`, por formato `16 * 1024 * 1024`, total `32 * 1024 * 1024`, silêncio `2 s`.
- Tipos de texto, nesta ordem: `["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"]`; imagem `image/png`; marca nossa `x-central-history`; marca de senha `x-kde-passwordManagerHint`.
- Metatipos do X11 ignorados: `TARGETS`, `MULTIPLE`, `TIMESTAMP`, `SAVE_TARGETS`, `DELETE`, `INSERT_PROPERTY`, `INSERT_SELECTION`.
- D-Bus: nome e interface `io.github.Aenes0.CosmicCentral.Clipboard`, objeto `/io/github/Aenes0/CosmicCentral/Clipboard`; métodos `Copy(t)`, `Pin(t)`, `Remove(t)`, `Clear()`; sinal `Changed()`.
- Repouso sem acordar: nenhum timeout no `dispatch` do calloop, nenhum `interval`/`sleep` em loop.
- Comentários e nomes no estilo do arquivo (inglês curto, frases tipo "Kept at most, besides the pinned.").
- Licença de corrigir o plano: se o código do brief não compilar, contradisser as assinaturas reais ou falhar nos testes do próprio brief, o plano pode estar errado: corrija para satisfazer o teste e relate a divergência; nunca altere o teste para acomodar implementação defeituosa.

Comando de teste (todas as tasks): `flatpak-spawn --host sh -c 'cd ~/Projetos/quick-settings && cargo test --release clipboard 2>&1 | tail -30'` (o perfil release já está compilado; `--release` evita recompilar tudo em debug). Build: `cargo build --release`.

## Review Focus

1. **App de origem que nunca fecha o pipe** (ou escreve devagar): o leitor não pode travar o serviço; prazo de silêncio devolve o que leu → teste `a_silent_pipe_gives_up_after_the_idle_time` (Task 2).
2. **Cópia gigante com vários formatos** (GIMP: png + bmp + jpeg grandes): o PNG/texto que vai para o histórico sobrevive ao corte total → teste `the_recorded_format_survives_the_total_limit` (Task 2).
3. **Senha copiada do KeePassXC**: não aparece no histórico nem volta depois de limpa → testes `a_secret_is_neither_recorded_nor_kept` (Task 2) e `a_secret_clears_what_was_kept` (Task 3, lógica `Keeper`).
4. **Serviço parado ou ainda não instalado** quando o applet pede uma ação: o applet não cai, só registra o aviso → teste `a_request_without_the_service_is_an_error_not_a_panic` (Task 4).
5. **Dois processos do serviço** (ativação D-Bus + systemd ao mesmo tempo): o segundo sai sem tocar no arquivo → teste `a_second_service_cannot_take_the_name` (Task 3).

---

### Task 1 [sonnet — refactor local, testável]: módulo `clipboard/` e `Store` serializado

**Files:**
- Delete: `src/clipboard.rs` (movido)
- Create: `src/clipboard/mod.rs` (tudo de hoje menos o `impl App`), `src/clipboard/panel.rs` (o `impl App { clipboard_panel, clipboard_card }` de hoje, sem mudanças de comportamento)
- `src/main.rs`: nada (`mod clipboard;` continua válido)

**Interfaces:**
- Produces (em `mod.rs`, `pub(crate)` onde não indicado):
  - `pub struct Store { dir: PathBuf }` com `Store::user() -> Store` (o `dir()` de hoje: `$XDG_STATE_HOME|~/.local/state` + `APP_ID`) e `Store::at(dir: PathBuf) -> Store` (testes).
  - `impl Store`: `fn load(&self) -> Vec<Entry>`, `fn change(&self, f: impl FnOnce(&mut Vec<Entry>)) -> Vec<Entry>`, `fn record(&self, mime: &str, bytes: &[u8]) -> Option<Vec<Entry>>` (None = nada gravado: texto vazio/inválido, falha ao gravar imagem), `fn touch(&self, id: u64) -> Vec<Entry>`, `fn pin(&self, id: u64) -> Vec<Entry>`, `fn remove(&self, id: u64) -> Vec<Entry>`, `fn clear(&self) -> Vec<Entry>`, `fn tidy(&self)` (corta para 15 + fixados e cria miniaturas que faltam: o `make_missing_thumbnails` de hoje, síncrono), `fn get(&self, id: u64) -> Option<Entry>`.
  - `pub fn load() -> Vec<Entry>` = `Store::user().load()` (o applet usa).
  - Mantidos: `Entry`, `shown`, `preview`, `make_thumbnail`, `thumbnail_path`, `remove_image`, `prune`, constantes, `TEXT_TYPES`, `IMAGE_TYPE`.
  - `change` segura um `static WRITING: Mutex<()>` do início do `load` até o `rename` do `save`.
- O código Wayland de hoje (`subscription`, `run`, `State`, `Dispatch`…) fica em `mod.rs` por enquanto, chamando `Store::user()`; sai na Task 4. O applet continua funcionando igual ao fim desta task.

- [ ] **Step 1: Mover** o `impl App` para `panel.rs` e o resto para `mod.rs`; `cargo test --release clipboard` passa com os 7 testes atuais (eles ficam em `mod.rs`).
- [ ] **Step 2: Testes que falham** (em `mod.rs`, `Store::at(scratch(...))`):

```rust
#[test]
fn record_puts_new_text_on_top_and_the_same_text_only_moves_up_keeping_the_pin() {
    let store = Store::at(scratch("record"));
    store.record("text/plain", b"a").unwrap();
    store.record("text/plain", b"b").unwrap();
    let id_a = store.load().iter().find(|e| e.text.as_deref() == Some("a")).unwrap().id;
    store.pin(id_a);
    let entries = store.record("text/plain", b"a").unwrap();
    assert_eq!(entries.iter().map(|e| e.text.clone().unwrap()).collect::<Vec<_>>(), ["a", "b"]);
    assert!(entries[0].pinned);
}

#[test]
fn blank_or_invalid_text_is_not_recorded() {
    let store = Store::at(scratch("blank"));
    assert!(store.record("text/plain", b"  \n").is_none());
    assert!(store.record("text/plain", &[0xff, 0xfe]).is_none());
    assert!(store.load().is_empty());
}

#[test]
fn a_picture_is_saved_with_its_thumbnail() {
    let dir = scratch("picture");
    let store = Store::at(dir.clone());
    let png_file = dir.join("src.png");
    png(&png_file, 300, 200);
    let entries = store.record(IMAGE_TYPE, &std::fs::read(&png_file).unwrap()).unwrap();
    let image = entries[0].image.clone().unwrap();
    assert!(image.starts_with(dir.join("clipboard")));
    assert!(thumbnail_path(&image).exists());
}

#[test]
fn pin_remove_clear_and_touch_change_the_file() { /* pin alterna; remove apaga a entrada e a imagem; clear deixa só os fixados; touch leva ao topo — cada um conferido com store.load() */ }

#[test]
fn changes_from_two_threads_are_all_kept() {
    let store = std::sync::Arc::new(Store::at(scratch("race")));
    let threads: Vec<_> = (0..2u64).map(|t| {
        let store = store.clone();
        std::thread::spawn(move || for i in 0..20 {
            store.change(|e| e.push(Entry { id: t * 100 + i, text: Some(format!("{t}-{i}")), image: None, pinned: true, time: 0 }));
        })
    }).collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    assert_eq!(store.load().len(), 40);
}
```

- [ ] **Step 3: Rodar** e ver falhar (`Store` inexistente).
- [ ] **Step 4: Implementar** `Store` movendo as funções livres de hoje para métodos (ids continuam `now()*1000 + len%1000`; o id não pode repetir um existente: se repetir, somar 1 até ficar livre).
- [ ] **Step 5: Rodar** `cargo test --release clipboard` (todos passam) e `cargo build --release` (sem warnings novos de `clipboard`).

---

### Task 2 [opus — I/O não bloqueante, cortes]: `keep.rs`, o que guardar e o leitor de pipes

**Files:**
- Create: `src/clipboard/keep.rs` (+ `mod keep;` em `mod.rs`)

**Interfaces:**
- Consumes: `TEXT_TYPES`, `IMAGE_TYPE`, `Entry` (Task 1).
- Produces:
  - `pub const HISTORY_MARK: &str = "x-central-history";`, `pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";`
  - `pub type Formats = HashMap<String, Arc<Vec<u8>>>;`
  - `pub fn is_ours(types: &[String]) -> bool`, `pub fn is_secret(types: &[String]) -> bool`
  - `pub fn readable(types: &[String]) -> Vec<String>` — sem metatipos X11, sem `HISTORY_MARK`, sem `SECRET_HINT`, sem repetidos, ordem preservada.
  - `pub fn primary(types: &[String]) -> Option<String>` — o 1º de `TEXT_TYPES` (na ordem de `TEXT_TYPES`) presente, senão `IMAGE_TYPE` se presente.
  - `pub fn entry_formats(entry: &Entry) -> Option<Formats>` — texto: os 5 `TEXT_TYPES` com os mesmos bytes; imagem: `IMAGE_TYPE` com os bytes do arquivo (None se não ler); sempre com `HISTORY_MARK` → vazio. None se nem texto nem imagem.
  - `pub struct Limits { pub per_format: usize, pub total: usize, pub idle: Duration }` com `impl Default` (16 MB, 32 MB, 2 s).
  - `pub fn read_all(pipes: Vec<(String, std::io::PipeReader)>, primary: Option<&str>, limits: &Limits) -> Formats`

`read_all`: põe todos os fds em não bloqueante (`libc::fcntl O_NONBLOCK`) e faz `libc::poll` sobre os abertos com timeout = `limits.idle`; `poll` devolvendo 0 = silêncio → para e devolve o que já terminou (EOF). Formato que passa de `per_format` é descartado (fd fechado). Se somar ao total passaria de `total`: sendo o primário, descartar os outros (maior primeiro) até caber; não sendo, descartar este. Formato com 0 bytes não entra. Só entram formatos que chegaram ao EOF.

- [ ] **Step 1: Testes que falham** (pipes de `std::io::pipe()`, escritores em threads):

```rust
#[test]
fn readable_drops_x11_meta_types_and_our_marks() {
    let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(readable(&t(&["TARGETS", "text/html", "UTF8_STRING", "x-central-history", "SAVE_TARGETS", "text/html"])), t(&["text/html", "UTF8_STRING"]));
}
#[test] fn primary_prefers_text_in_its_order_then_png() // ["image/png","STRING","text/plain"] → "text/plain"; ["image/jpeg","image/png"] → "image/png"; ["text/html"] → None
#[test] fn a_secret_is_neither_recorded_nor_kept() // is_secret(["text/plain","x-kde-passwordManagerHint"]) == true; readable(...) não contém a marca
#[test] fn entry_formats_offers_text_in_every_text_type_with_our_mark() // 5 tipos + HISTORY_MARK, bytes iguais
#[test] fn pipes_written_out_of_order_do_not_block() // 3 pipes; a thread escreve 200 KB no 3º, depois no 1º, depois no 2º (cada um > 64 KB de buffer), fecha; read_all devolve os 3 inteiros
#[test] fn a_format_over_its_limit_is_dropped_the_others_kept() // Limits{per_format: 1000, ..}: 2000 bytes em "a", 10 em "b" → só "b"
#[test] fn the_recorded_format_survives_the_total_limit() // Limits{per_format: 1000, total: 1500}: "image/bmp" 900 bytes escrito primeiro, depois "image/png" 900; primary=Some("image/png") → contém "image/png" com 900, não contém "image/bmp"
#[test] fn a_silent_pipe_gives_up_after_the_idle_time() // Limits{idle: 200ms}: "a" fechado com 5 bytes, "b" com a ponta de escrita mantida aberta e muda; read_all volta em < 1 s com só "a"
```

- [ ] **Step 2: Rodar** e ver falhar.
- [ ] **Step 3: Implementar** as funções acima.
- [ ] **Step 4: Rodar** `cargo test --release clipboard` — tudo passa.

---

### Task 3 [opus — concorrência Wayland+D-Bus]: `service.rs` e o modo `--clipboard`

**Files:**
- Create: `src/clipboard/service.rs` (+ `pub mod service;` em `mod.rs`)
- Modify: `src/main.rs` — uma linha de despacho depois do `tracing_subscriber…init()`, junto aos outros modos: `if std::env::args().any(|arg| arg == "--clipboard") { clipboard::service::main() }` (`service::main() -> !`, sai com `std::process::exit`). Reler `main.rs` antes: outra frente acrescentou `--tray` nele.

**Interfaces:**
- Consumes: `Store` (Task 1); `keep::*` (Task 2).
- Produces:
  - `pub const NAME: &str = "io.github.Aenes0.CosmicCentral.Clipboard"; pub const PATH: &str = "/io/github/Aenes0/CosmicCentral/Clipboard";` (o cliente da Task 4 usa)
  - `pub fn main() -> !`
  - `pub struct Keeper { kept: Option<Formats> }` — lógica pura, testável:
    - `fn offered(&mut self, types: &[String]) -> Offer` onde `enum Offer { Ours, Secret, Read { types: Vec<String>, primary: Option<String> } }`; `Secret` esvazia `kept`.
    - `fn received(&mut self, formats: Formats)` — vazio não substitui.
    - `fn copied(&mut self, formats: Formats)` — `Copy(id)` substitui.
    - `fn emptied(&self) -> Option<Formats>` — o que repor (com `HISTORY_MARK`), se houver.
  - `pub struct Service { store: Arc<Store>, requests: calloop::channel::Sender<Request> }` com `#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Clipboard")]`: `async fn copy(&self, id: u64)`, `async fn pin(&self, id: u64)`, `async fn remove(&self, id: u64)`, `async fn clear(&self)`, todos `-> zbus::fdo::Result<()>`, e `#[zbus(signal)] async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>`. Cada método grava e então emite `Changed` (`emitter: SignalEmitter<'_>` via `#[zbus(signal_emitter)]`).
  - `pub enum Request { Copy { id: u64, done: futures::channel::oneshot::Sender<()> } }`
  - `pub async fn serve(conn: zbus::connection::Builder<'_>, service: Service) -> zbus::Result<zbus::Connection>` — `.name(NAME)?.replace_existing_names(false).serve_at(PATH, service)?.build()`.

Desenho do processo:
- Thread principal: calloop. `State` como o de hoje sem `writer`/trava, mais `keeper: Keeper`, `store: Arc<Store>`, `changed: tokio::sync::mpsc::UnboundedSender<()>`. `event_loop.dispatch(None, …)` num loop.
- `Selection { id: Some(offer) }`: `keeper.offered(types)` → `Read` com `readable` não vazio: para cada tipo `offer.receive(tipo, fd)` (pipes de `std::io::pipe()`), `conn.flush()`, e uma thread roda `keep::read_all(...)` e manda `(primary, Formats)` por `calloop::channel` ao loop; no loop: `keeper.received(formats.clone())`, `store.record(primary, bytes)` se houver primário nos formatos, e `changed.send(())` se gravou. `offer.destroy()` sempre.
- `Selection { id: None }`: `keeper.emptied()` → `set_selection` com uma fonte nossa (o `State::copy` de hoje generalizado para `Formats`).
- `Request::Copy`: `store.get(id)` → `keep::entry_formats` → `keeper.copied(f.clone())`, `set_selection`, `store.touch(id)`, `done.send(())`.
- Thread 2: `tokio::runtime::Builder::new_current_thread().enable_all()`; `serve(Builder::session()?, …)`; uma task que recebe de `changed` e emite `Changed` (`conn.object_server().interface::<_, Service>(PATH).await?.signal_emitter()`).
- Partida: `store.tidy()` antes de tudo. Nome tomado → `tracing::error!` e `exit(1)`. Conexão Wayland caiu (`dispatch` devolve erro) → `exit(1)`.

- [ ] **Step 1: Testes que falham** em `service.rs`:

```rust
#[test] fn a_secret_clears_what_was_kept() // Keeper com kept=Some; offered([.., SECRET_HINT]) == Secret; emptied() == None
#[test] fn our_own_offer_changes_nothing() // offered([.., HISTORY_MARK]) == Ours; kept igual
#[test] fn an_empty_read_keeps_the_last_copy() // received(vazio) não substitui
#[test] fn copy_again_is_what_comes_back() // received(A); copied(B); emptied() tem os bytes de B e HISTORY_MARK
#[tokio::test] async fn methods_write_the_file_and_signal_changed()
// sobe `dbus-daemon --session --nofork --print-address` (pula com eprintln! se /usr/bin/dbus-daemon não existir),
// Store::at(scratch) com 2 entradas; serve(Builder::address(addr)?, …); outra conexão assina Changed;
// chama Pin(id) e Remove(id2) por um proxy; confere o arquivo e que chegaram 2 sinais.
// Copy: uma thread de teste recebe o Request do canal e responde done; confere que o item foi ao topo.
#[tokio::test] async fn a_second_service_cannot_take_the_name() // 2º serve no mesmo bus → Err
```

- [ ] **Step 2: Rodar** e ver falhar.
- [ ] **Step 3: Implementar** `Keeper`, `Service`, `serve`, o loop e `main`; despacho em `main.rs`.
- [ ] **Step 4: Rodar** `cargo test --release clipboard` e `cargo build --release`.
- [ ] **Step 5: Fumaça sem instalar** (não toca no painel nem na área de transferência; só confere que sobe e cede):  
  `flatpak-spawn --host sh -c 'cd ~/Projetos/quick-settings && timeout 3 target/release/cosmic-ext-applet-central --clipboard; echo saiu=$?'` → `saiu=124` (rodou até o timeout, nome tomado) e, rodando dois ao mesmo tempo, o segundo sai com `1` e mensagem de nome tomado.  
  **Atenção:** enquanto roda, este processo já repõe a área de transferência se ela ficar vazia e grava cópias no histórico real. 3 s é aceitável; não deixar rodando.

---

### Task 4 [sonnet — contrato pronto]: applets viram clientes

**Files:**
- Modify: `src/clipboard/panel.rs` (cliente), `src/clipboard/mod.rs` (remove o Wayland antigo: `subscription`, `run`, `writer`, `State`, `Source`, `OfferData`, `receive`, os `Dispatch`, `Event::Ready`, `Event::History`, `Request` antigo), `src/app.rs` (linhas de `clipboard` listadas abaixo).

**Interfaces:**
- Consumes: `service::NAME`, `service::PATH` (Task 3); `load()` (Task 1).
- Produces (em `panel.rs`):
  - `#[zbus::proxy(interface = "io.github.Aenes0.CosmicCentral.Clipboard", default_service = "io.github.Aenes0.CosmicCentral.Clipboard", default_path = "/io/github/Aenes0/CosmicCentral/Clipboard")] trait History { fn copy(&self, id: u64) -> zbus::Result<()>; fn pin(...); fn remove(...); fn clear(&self) -> zbus::Result<()>; #[zbus(signal)] fn changed(&self) -> zbus::Result<()>; }`
  - `pub enum Action { Copy(u64), Pin(u64), Remove(u64), Clear }`
  - `pub async fn request(conn: &zbus::Connection, action: Action) -> zbus::Result<()>` e `pub async fn ask(action: Action)` (conexão de sessão guardada num `tokio::sync::OnceCell`, como `notifications.rs`; erro → `tracing::warn!`).
  - `pub fn subscription() -> Subscription<()>` — um `()` por sinal `Changed` (reconecta com espera de 5 s se o bus cair; nada de polling no caminho normal).
- `app.rs`:
  - `Message::Clipboard(crate::clipboard::Event)` → `Message::ClipboardChanged`; tratamento: `self.clipboard = crate::clipboard::load()`.
  - Campo `clipboard_tx` sai.
  - `CopyEntry/PinEntry/DeleteEntry/ClearClipboard(id)` → `Task::future(async move { crate::clipboard::ask(Action::…).await; cosmic::action::app(Message::ClipboardChanged) })`.
  - `init`: sai `crate::clipboard::make_missing_thumbnails()`.
  - `subscription`: `crate::clipboard::subscription().map(|()| Message::ClipboardChanged)`.
  - `load()` ao abrir o popup fica como está.

- [ ] **Step 1: Teste que falha** em `panel.rs`:

```rust
#[tokio::test]
async fn a_request_without_the_service_is_an_error_not_a_panic() {
    // bus privado (dbus-daemon como na Task 3; pula se não houver) sem ninguém com o nome
    let conn = zbus::connection::Builder::address(addr).unwrap().build().await.unwrap();
    assert!(request(&conn, Action::Pin(1)).await.is_err());
}
```

- [ ] **Step 2: Rodar** e ver falhar.
- [ ] **Step 3: Implementar** cliente e mudanças no `app.rs`; remover o Wayland antigo de `mod.rs`.
- [ ] **Step 4: Verificar** `cargo test --release clipboard`, `cargo build --release` sem warnings novos, e `grep -rn "central-clipboard.lock\|ext_data_control" src/ | grep -v src/clipboard/service.rs` → vazio.

---

### Task 5 [haiku — arquivos de dados]: units systemd e D-Bus no `install.sh`

**Files:**
- Create: `data/central-clipboard.service.in`, `data/io.github.Aenes0.CosmicCentral.Clipboard.service.in`
- Modify: `install.sh` (reler antes: outra frente o edita)

Conteúdo exato:

```ini
# data/central-clipboard.service.in
[Unit]
Description=Central: histórico da área de transferência
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=dbus
BusName=io.github.Aenes0.CosmicCentral.Clipboard
ExecStart=@bindir@/cosmic-ext-applet-central --clipboard
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
```

```ini
# data/io.github.Aenes0.CosmicCentral.Clipboard.service.in
[D-BUS Service]
Name=io.github.Aenes0.CosmicCentral.Clipboard
Exec=@bindir@/cosmic-ext-applet-central --clipboard
SystemdService=central-clipboard.service
```

`install.sh`, depois do ícone: gerar os dois com o mesmo `sed "s|@bindir@|$prefix/bin|"` em `$prefix/share/systemd/user/central-clipboard.service` e `$prefix/share/dbus-1/services/io.github.Aenes0.CosmicCentral.Clipboard.service` (`mkdir -p` antes); depois, só se `[ "$prefix" = "$HOME/.local" ] && [ ! -f /.flatpak-info ]`: `systemctl --user daemon-reload && systemctl --user enable central-clipboard.service && systemctl --user restart central-clipboard.service || true`.

- [ ] **Step 1: Verificar** `sh -n install.sh` (sem saída) e, gerando a unit num diretório de rascunho com `sed`, `flatpak-spawn --host systemd-analyze --user verify <arquivo>` sem erros (avisos sobre o binário inexistente no rascunho são aceitáveis se o caminho for de rascunho; usar `@bindir@` → `$HOME/Projetos/quick-settings/target/release` para não haver aviso).
- [ ] **Não** rodar `install.sh`.

---

### Task 6 [controller, com o OK do usuário]: instalar e verificar no sistema

- [ ] Pedir o OK ao usuário (instalar recarrega o painel). Com o OK: `.dev/recarregar.sh`.
- [ ] `systemctl --user status central-clipboard.service` ativo; `pgrep -fc 'cosmic-ext-applet-central$'` = 3 applets e nenhum deles com conexão de data-control (`ls -l /proc/<pid>/fd` não basta; conferir pelo comportamento abaixo).
- [ ] Copiar texto e imagem → aparecem no popup dos 3 monitores (com o popup aberto, ao vivo).
- [ ] Copiar de novo, fixar, apagar, limpar.
- [ ] Copiar de um app, fechar o app, colar → cola.
- [ ] Copiar de novo pelo histórico, recarregar o painel, colar → cola.
- [ ] Repouso: `voluntary_ctxt_switches` em `/proc/<pid>/status` (todas as threads: `/proc/<pid>/task/*/status`) parado por 60 s sem copiar nada → sem aumento; `VmRSS` registrado.

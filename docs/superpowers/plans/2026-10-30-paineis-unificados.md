# Plano — painéis unificados (`--panels`)

Spec: `docs/superpowers/specs/2026-10-30-paineis-unificados-design.md`. Sem commits.
Molde de referência (somente leitura): `~/Projetos/start-menu/src/main.rs`,
`src/menu.rs` (`show`/`hide`, `get_layer_surface`), `src/menu/service.rs` (instância única por D-Bus).

## Fase 0 — Linha de base (medir antes de mexer)
- Registrar tempo de abrir Central e Monitor hoje (`COSMIC_QS_PREVIEW`/`COSMIC_MONITOR_PREVIEW`,
  logs com `Instant`) e o que bloqueia. Salvar em `docs/superpowers/plans/2026-10-30-paineis-medicoes.md`.

## Fase 1 — Serviço e host vazio
- `src/panels/service.rs`: D-Bus `Toggle(which: s, output: s)`, `Close()`; `toggle_running(which, output) -> bool`
  assíncrono/não bloqueante para o applet; spawn do `--panels` quando não responde.
- `src/panels/mod.rs`: `Panels` (cosmic::app::Application, sem janela principal), mensagens
  `Toggle`, `Closed`, superfície layer-shell em tela cheia na saída pedida; Esc/clique fora fecha.
- `main.rs`: modo `--panels` (`TOKIO_WORKER_THREADS=1`, instância única como o `--menu`).
- Teste: `busctl --user call … Toggle` abre/fecha a superfície vazia.

## Fase 2 — Hospedar Central e Monitor
- Wrapper `Panels { central: App, monitor: Monitor, core }` com `Message::{Central, Monitor, ..}`;
  `Task::map`/`Element::map`/`Subscription::map`. Desacoplar o que depende de `core.applet`/`main_window_id`
  (modo hospedado: sem painel-pai, `anchor` vindo da config do painel/saída).
- Trocar `TogglePopup`/`close_popup`/`on_close_requested` por abrir/fechar a superfície quando hospedado.
- Visual idêntico: mesma largura (`app::WIDTH`), `popup_limits`, `autosize` do Monitor.
- Subscriptions só com a superfície aberta.

## Fase 3 — Applets da barra viram gatilhos
- `TogglePopup` dos applets → `panels::service::toggle(which, output)` assíncrono (Task), sem popup.
- `COSMIC_QS_PREVIEW*`/`COSMIC_MONITOR_PREVIEW*` passam a chamar o host.
- Restante do applet (ícones, energia) intacto.

## Fase 4 — Isolar coleta pesada
- NVML/`/proc`/DDC/scan/`clipboard::load` em `spawn_blocking` + `tokio::time::timeout`; resultado por mensagem.
- Primeiro quadro não espera dados.

## Fase 5 — Instalação e verificação
- `install.sh` e `data/`: instalar e iniciar o `--panels` (unit de usuário ou ativação D-Bus, como o clipboard).
- `.dev/recarregar.sh` coerente com o novo fluxo (documentar que reiniciar o painel ainda reinicia os applets).
- Rodar: build, testes, medição de abertura, teste `kill -STOP`, repouso 60 s. Mostrar saídas.
- AVISAR o usuário antes de reinstalar (reinstalar reinicia o painel inteiro — ver memória `painel-recarga`).

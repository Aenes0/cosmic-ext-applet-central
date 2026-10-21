# Plano — bloco "Câmera" (Iriun + adb)

Spec: `docs/superpowers/specs/2026-10-21-camera-iriun-design.md`. Ninguém commita; tudo fica no working tree.
Comandos do host (cargo, python3, systemctl, adb) via `flatpak-spawn --host sh -c '…'`.

## T1 [opus — supervisão/estado] iriun-adb: serviço supervisor
Arquivos: `~/Projetos/iriun-adb/src/iriun_adb/{session.py,pc.py,status.py,phone.py,__main__.py}`,
`tests/test_session.py`, `tests/test_pc_status.py`.
- `Session`: mantém o estado anterior da 1ª conexão; `begin` / `lost` / `keep_alive` / `end`; flag `failed`
  (PIN) sem nova tentativa.
- `PcIriun`: abre/encontra/encerra o `iriunwebcam` (pids por `/proc/*/comm`).
- `StatusWriter`: JSON atômico do contrato, só grava quando muda.
- `Supervisor.tick` / `shutdown` substituem o `Watcher`.
- Teste: pytest com fakes; verificação: `python3 -m pytest -q` verde.

## T2 [haiku — unit fixa] iriun-adb: unit + install
`systemd/iriun-camera.service` (`Restart=on-failure`, `KillMode=mixed`, `TimeoutStopSec=20`,
`PYTHONPATH=%h/Projetos/iriun-adb/src`), `install.sh` (copia a unit, `daemon-reload`; não habilita no boot).
Verificação: `systemd-analyze --user verify`.

## T3 [opus — integração multi-arquivo UI] quick-settings: bloco + página
Arquivos: `src/tiles.rs`, `src/services.rs`, `src/app.rs`, `src/pages.rs`, `src/i18n.rs` (e onde mais o
padrão exigir, ex.: lista padrão de blocos/ícones da barra, `Page::from_name`).
Padrão a copiar: `Tile::HotEdge` (disponibilidade, toggle, seta) e páginas existentes (`Page::DoNotDisturb`).
Teste: unitário do parser do `status.json` e de `Tile::Camera` id/from_id em `#[cfg(test)]`;
verificação: `cargo build --release` e `cargo test` verdes.

## T4 [fable — revisão] revisão das duas partes; T5 teste real com o celular (usuário + controller).

T1/T2 e T3 são independentes (só o contrato da spec as liga): T3 roda em paralelo.

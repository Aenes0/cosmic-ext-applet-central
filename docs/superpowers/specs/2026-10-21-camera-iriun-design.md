# Bloco "Câmera": Iriun no PC + Galaxy S8+ via adb

## Intenção
Usar o Galaxy S8+ (no suporte, no cabo USB) como webcam pelo Iriun sem abrir nada à mão: um bloco
"Câmera" na Central liga tudo e desliga tudo. O celular fica com o painel apagado durante o uso,
sem bloquear, e volta ao estado em que estava.

## Partes
1. **`~/Projetos/iriun-adb`** (Python, sem dependências) — serviço `iriun-camera.service`
   (systemd `--user`). Iniciar = abrir o Iriun do PC + fluxo do celular; parar = desfazer tudo.
2. **Bloco `camera` na Central** (`quick-settings`, Rust/libcosmic) — liga/desliga o serviço,
   página "Câmera" com o status e "Reiniciar".

## Contrato entre as partes
- Unit: `iriun-camera.service`, instalada em `~/.config/systemd/user/`.
  - Bloco disponível ⇔ o arquivo da unit existe.
  - Ligado ⇔ `systemctl --user is-active iriun-camera.service` imprime `active` ou `activating`.
  - Ligar `systemctl --user start …`, desligar `stop …`, Reiniciar `restart …`.
- Status: `~/.local/state/iriun-adb/status.json`, gravado de forma atômica a cada mudança:
  ```json
  {"state": "starting|waiting_phone|active|error|stopped",
   "pc_iriun": true, "phone_connected": true, "phone_iriun": true, "screen_off": true,
   "error": null, "pc_window": "hidden|shown|null", "updated": 1761080000}
  ```
  `error` é texto pronto para mostrar (pt-BR) ou `null`. Com o serviço inativo, o bloco ignora o arquivo;
  ativo, um `stopped` é o arquivo da execução anterior e conta como "carregando".
- Janela do Iriun do PC: mostrar `systemctl --user kill --kill-whom=main -s SIGUSR1 iriun-camera.service`,
  esconder `-s SIGUSR2`.

## Comportamento do serviço
- Ao iniciar: abre `iriunwebcam` com `QT_QPA_PLATFORM=xcb` (XWayland) e retira a janela pelo X11
  (`XWithdrawWindow`) assim que ela aparece: some da tela, da Taskbar e da Dock. `XMapRaised` traz a mesma
  janela de volta. (O Iriun não tem bandeja no COSMIC; minimizar deixava ele na barra.)
- Status inicial `starting` gravado antes do 1º ciclo; o servidor adb sobe num escopo próprio (`ExecStartPre`).
- A cada 2 s:
  - Iriun do PC morreu → reabre.
  - Celular ausente → `waiting_phone`; ao voltar, refaz o fluxo.
  - Fluxo do celular, uma vez por conexão:
    1. guarda o estado (tela, keyguard, `deviceLocked` do `dumpsys trust`), só na primeira vez da execução;
    2. acorda; se houver keyguard, `wm dismiss-keyguard`;
    3. se `deviceLocked=1`, digita o PIN do chaveiro (`secret-tool lookup service iriun-adb account galaxy-s8`)
       pelo stdin do `adb shell`;
    4. `am start` do Iriun;
    5. `scrcpy --no-window --no-video --no-audio --turn-screen-off --stay-awake`.
  - Durante o uso: se o scrcpy morrer, reabre; se o Iriun do celular morrer, reabre.
- PIN errado ou ausente, ou erro do adb logo depois de digitar o PIN → `error`, notificação, **sem nova
  tentativa** até reiniciar (evita bloqueio por tentativas). A falha fica num marcador em disco
  (`~/.local/state/iriun-adb/pin-failed`), apagado só num desligar/Reiniciar limpo: um reinício automático
  do systemd depois de uma queda não redigita o PIN.
- scrcpy que morre logo 3 vezes seguidas → erro e nova tentativa em 30 s.
- Ao parar (SIGTERM):
  1. encerra o scrcpy;
  2. `am force-stop` do Iriun no celular;
  3. devolve o estado do celular: aceso e desbloqueado → acorda; senão `KEYCODE_SLEEP`;
  4. encerra o Iriun do PC;
  5. grava `stopped`.
- `Restart=on-failure`, `KillMode=mixed`, `TimeoutStopSec=60` (o serviço limpa antes de o systemd matar o resto).

## Bloco e página
- Bloco grande/pequeno como os outros, ícone `camera-web-symbolic`, nome "Câmera"/"Camera"/"Caméra".
- Clique: liga/desliga. Seta ou clique direito: página "Câmera".
- Página:
  - uma linha por parte (Iriun no PC, Celular conectado, Iriun no celular, Tela do celular apagada) com ✓/—;
  - o estado geral e o último erro;
  - "Mostrar/Esconder janela do Iriun" (pelo `pc_window`) e "Reiniciar".
  - Atualiza enquanto aberta.

## Segurança
O PIN nunca é argumento de processo, nunca vai para log nem para mensagens de erro; só dígitos (4–16).

## Fora do escopo
Detectar o Iriun aberto à mão; Wi-Fi/adb sem cabo.

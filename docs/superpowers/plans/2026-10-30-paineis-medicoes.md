# Medições — painéis unificados (`--panels`)

Máquina: CachyOS, COSMIC, 3 saídas (DP-3 ultrawide, DP-1, HDMI-A-1), GPU NVIDIA, renderer tiny-skia.
Desktop em uso durante as medições (outros builds rodando, carga ~10 em 32 threads): números com ruído.
Tempos com `COSMIC_PANELS_TIMING=1` (`src/timing.rs`).

## Fase 0 — linha de base (popup xdg no applet)
Applet rodado avulso com `COSMIC_QS_PREVIEW=main` / `COSMIC_MONITOR_PREVIEW=1`, 3 rodadas.

| | pedido → 1ª view |
|---|---|
| Central | 0,6 – 1,1 ms |
| Monitor | 0,3 – 0,5 ms |

O "mostrado" (configure do popup) não foi mensurável: nem `window::size` nem eventos de popup/frame
chegam à subscription para popups xdg. O trabalho síncrono no `update` não era o gargalo.

## Depois — host `--panels`, superfície layer-shell (pedido → configure da superfície)

| | frio (processo novo) | morno |
|---|---|---|
| Central | ~440 ms do exec (init ~150 ms + 284 ms) | 38 – 102 ms |
| Monitor | ~270 ms do exec | 43 – 86 ms |

## Tela cheia transparente × superfície do tamanho do painel
5 aberturas de cada, janela de 0,8 s após o pedido (CPU = todas as threads; "fila" = espera na runqueue da
thread principal do cosmic-comp).

| variante | painel | mostrado (ms) | CPU host (ms) | CPU comp (ms) |
|---|---|---|---|---|
| tela cheia | Central | 46 – 147 | 212 – 426 | 125 – 669 |
| tela cheia | Monitor | 66 – 85 | 436 – 571 | 566 – 684 |
| tamanho do painel | Central | 45 – 158 | 216 – 416 | 130 – 501 |
| tamanho do painel | Monitor | 5 – 134 | 333 – 615 | 486 – 813 |

Sem diferença clara de custo entre as duas. Escolhido o **tamanho do painel** por isolamento: com o host
parado (`kill -STOP`), uma superfície em tela cheia deixaria a saída inteira sem clique; a do tamanho do
painel só prende o retângulo dele. Clique fora fecha via perda de teclado (`OnDemand` → `Unfocused`) e Esc.
`COSMIC_PANELS_SURFACE=screen` volta à tela cheia para comparar. Blur: o host pede `Auto::Popup`, que não
borra layer surfaces; desligar de vez não mudou nada nas medições.

## Causa do lag do cursor (Central)
Amostragem do estado das threads do cosmic-comp (`/proc/PID/task/*/stat`, 1 ms):

| situação | threads em D (sono não interrompível) | maior trecho em D |
|---|---|---|
| repouso | — | 0 ms |
| `ddcutil getvcp 10` (display 1, 582–637 ms) | `surface-DP-3`, `surface-HDMI-A-1` | 232 ms |
| `ddcutil getvcp 10` (display 2, 606–610 ms) | `surface-DP-3`, `surface-HDMI-A-1` | 235 ms |

A leitura DDC/CI (i²c pela GPU) prende as threads de render por saída do compositor no lock do driver
NVIDIA (`os_acquire_rwlock_*`): o cursor não anda nessas saídas. O Central lia o brilho do monitor
principal por DDC **ao abrir**, se a última leitura tinha mais de 120 s — daí "algumas aberturas" e só o
painel da direita. CPU e IO não explicam: a thread principal do compositor não ficou em fila nem em D.

Depois (abrir não lê DDC; usa o último nível lido/definido e os monitores com DDC guardados em
`$XDG_STATE_HOME/cosmic-central/ddc-{levels,displays}`, que agora persistem entre sessões; sem nada
guardado, uma detecção em segundo plano ~45 s após o host subir, nunca com painel aberto):
6 aberturas do Central e 6 do Monitor, 1 s cada — nenhum trecho em D nas aberturas; um único de 145 ms
num fechamento (lock do driver, sem `ddcutil` rodando — outro cliente da GPU, não atribuído).

## Isolamento (host em unidade transitória com os limites da unidade real)
`systemd-run --user -p Type=notify -p WatchdogSec=20 -p Nice=10 -p CPUWeight=20 -p IOWeight=20
-p MemoryHigh=200M -p MemoryMax=400M -p TasksMax=64 -p OOMScoreAdjust=500`

- `kill -STOP` com o Central aberto: pedido estilo applet (sem resposta) volta em 2,5 ms; cosmic-comp e
  cosmic-panel em `S`; `cosmic-randr list` ≤ 3,7 ms; nenhuma thread do compositor presa > 17 ms.
- Watchdog: parado, o systemd matou e reiniciou o host ("Watchdog timeout (limit 20s)", novo PID).
- Carga pesada (laço de CPU em todos os núcleos + 600 MB de escrita direta), 4 s:
  com os limites: fila da thread do compositor 1073 ms, D 0 ms, `cosmic-randr` ≤ 13,9 ms;
  sem limites: fila 1509 ms. Os limites reduzem, mas um laço em todos os núcleos ainda atrasa o
  compositor — o host real não faz isso (coleta em threads `SCHED_IDLE`).
- Cursor: não há como medir o cursor daqui (sem dispositivo de entrada virtual); os números acima são o
  substituto (threads do compositor em D/fila).

## Repouso
Host com os painéis fechados, 60 s: **0** trocas de contexto (todas as threads), 7 threads, RSS 60 MB.
Sob o watchdog do systemd, o host acorda a cada 10 s para o batimento (exigido pelo watchdog).

## Unidade assumindo o nome (host à mão já rodando)
Host à mão dono do nome → `systemd-run --user -p Type=notify -p WatchdogSec=20 … --panels`: unidade
`active/running`, `Result=success`, o nome passa ao PID da unidade e o host à mão sai; Toggle/Close pela
unidade OK; um segundo `--panels` sem argumentos sai com rc 0.

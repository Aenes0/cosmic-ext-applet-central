# Painéis unificados — um processo isolado para o Central e o Monitor

Aprovado pelo usuário em 2026-10-30 (execução direto em `main`, sem commit).

## Problema
Os painéis da direita (Central) e da esquerda (Monitor) abrem como popup xdg dentro do
processo do applet, filho do cosmic-panel (6 processos: 2 applets × 3 monitores). Abrir é lento
e, às vezes, trava o PC todo. A causa não foi medida; a hipótese é que trabalho bloqueante ao abrir
(NVML por dlopen, /proc, DDC, scan de Wi-Fi, leitura do clipboard) segura o popup, que prende o
grab do compositor, e que um applet travado derruba a barra junto.

## Objetivo
- **Um** processo `--panels` hospeda os dois painéis, para todos os monitores.
- O visual dos cards, tiles e painéis fica **idêntico** (mesmos widgets).
- Se o `--panels` travar, só ele fica lento: barra e compositor seguem responsivos.
- Em repouso (painéis fechados) o `--panels` não redesenha, não amostra e não consome CPU.
- Abrir é rápido: a superfície aparece antes de qualquer coleta pesada; os dados entram em
  seguida, sem bloquear o quadro.

## Não-objetivos
- Mudar o visual ou o conteúdo dos cards.
- Mexer no cosmic-panel, no compositor ou nos serviços `--clipboard`/`--notifications`.
- Reescrever a coleta de dados do Monitor/Central (só mover o ponto de abertura e isolar o que bloqueia).

## Anti-objetivos
- Um travamento do `--panels` não pode segurar o grab/entrada do compositor nem a barra.
- Nenhuma chamada D-Bus síncrona do applet da barra para o `--panels`.

## Desenho
1. `--panels`: aplicativo `cosmic::app` (como o `--menu` do start-menu: `no_main_window`,
   `exit_on_close(false)`, `transparent`), instância única por D-Bus
   (`io.github.Aenes0.CosmicCentral.Panels`, métodos `Toggle(which, output)` e `Close`),
   oculto entre usos.
2. Abrir = uma superfície layer-shell **do tamanho do painel** (padrão), na saída pedida
   (`Layer::Top`, teclado `OnDemand`), ancorada junto à barra (Central na ponta final, Monitor na
   inicial). Fecham: Esc, o teclado ir para outro lugar (clique numa janela; `Unfocused`) e o clique
   no applet. Fechar = destruir a superfície. `COSMIC_PANELS_SURFACE=screen` volta à tela cheia
   transparente do start-menu (clique em qualquer lugar fecha), só para comparar: medido, o custo é
   parecido, mas travado o host prenderia a saída inteira.
   Limitação conhecida (não verificada): clique no papel de parede ou em área vazia da barra pode não
   fechar, se o compositor não tirar o teclado do painel nesses casos; não há forma barata e segura de
   cobrir sem voltar à tela cheia.
3. O `--panels` embute os estados existentes `App` e `Monitor` num wrapper (`Message::Central(..)`,
   `Message::Monitor(..)`) e reaproveita `view`/`update`. A criação/destruição do popup xdg é
   trocada por criação/destruição da superfície layer-shell no modo hospedado.
4. Os applets da barra continuam como estão (ícones, medidor de energia), mas clicar manda
   `Toggle` por D-Bus, assíncrono, sem esperar resposta, e **não abre mais popup**. Se o
   `--panels` não responde, o applet o inicia (`cosmic-ext-applet-central --panels`).
5. Subscriptions do hospedado só vivem com a superfície aberta (exceto o necessário para
   manter o D-Bus de instância única).
6. Coleta pesada (NVML, /proc, clipboard) numa thread própria `SCHED_IDLE`, com timeout; DDC e
   Wi-Fi com timeout; o resultado chega por mensagem. Nada disso roda no caminho do primeiro quadro.
7. **DDC nunca ao abrir**: uma leitura DDC prende as threads de render do compositor (medido: até
   ~235 ms, o cursor junto). Abrir usa os níveis lidos/definidos por último e os monitores com DDC
   guardados em `$XDG_STATE_HOME/cosmic-central/ddc-{displays,levels}`; a página de monitores lê ao
   ser aberta. Sem nada guardado, o host detecta uma vez, ~45 s depois de subir, nunca com painel
   aberto ou aberto há menos de 10 s. `ddcutil` roda em `SCHED_IDLE`.
8. Isolamento de recursos: unidade `central-panels.service` (Type=notify, watchdog de 20 s batido pelo
   loop da UI, Nice=10, CPUWeight=20, IOWeight=20, MemoryHigh=200M, MemoryMax=400M, TasksMax=64,
   OOMScoreAdjust=500). O applet inicia a unidade (`systemctl --user start --no-block`) e espera o
   nome no barramento até 15 s, fora da UI; o host da unidade assume o nome de um iniciado à mão,
   que sai.

## Verificação
- `cargo build --release` e `cargo test` verdes (no host: `flatpak-spawn --host`).
- Medição antes/depois do tempo de abrir (clique → primeiro quadro) para cada painel.
- Teste de isolamento: `kill -STOP` no `--panels` com painel aberto; a barra, o clique em outros
  applets e o cursor seguem respondendo.
- Repouso: `--panels` sem wakeups relevantes em 60 s (`/proc/PID/status` trocas de contexto).

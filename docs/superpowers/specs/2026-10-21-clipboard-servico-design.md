# Histórico da área de transferência num processo próprio

## Intenção
Hoje cada um dos 3 applets do Central (um por monitor) abre a sua conexão `ext-data-control` com o
compositor, e o que pega a trava `$XDG_RUNTIME_DIR/central-clipboard.lock` grava o histórico. Quando o
painel recarrega (21 processos), a coleta para e volta, e o que um applet repôs na área de transferência
("copiar de novo") some junto com ele. O usuário quer applets com processos individualizados e
consumo baixíssimo. A meta aqui é **robustez e independência**, não velocidade (a lentidão do popup já
foi resolvida: 15 itens, 200 caracteres por cartão).

Além disso (pedido do usuário, 2026-10-21): **o último item copiado continua colável depois que o app
de onde ele veio fecha**, como no Windows.

## Metas
- Um processo só, `cosmic-ext-applet-central --clipboard`, rodando como serviço `systemd --user`:
  uma conexão com o compositor, que não para quando o painel recarrega.
- Os applets não falam mais com o compositor sobre a área de transferência: leem o arquivo e pedem as
  ações ao serviço por **D-Bus** (escolha do usuário).
- O serviço é o **único** que grava `clipboard.json` e as imagens: acaba a corrida entre os 3 applets.
- Consumo em repouso ~zero: nenhum timer, nenhum polling. O processo só acorda com um evento do
  compositor ou uma chamada D-Bus.
- Manter colável: quando o app dono da cópia some, o serviço repõe o mesmo conteúdo.
- Sem regressão: texto, imagens com miniatura, fixar, apagar, limpar, copiar de novo (marcado
  `x-central-history`, que não volta como item novo), "o mesmo texto de novo só sobe", 15 itens além dos
  fixados, cortes de tamanho.

## Fora do escopo
- Seleção primária (botão do meio). Só a área de transferência normal.
- Mudar o painel visual (cartões, layout, textos).
- Guardar no disco formatos além de texto e PNG (o histórico continua como hoje). Os outros formatos
  ficam só na memória, para o "manter colável".

## Anti-metas (seria falha mesmo funcionando)
- Processo que acorda periodicamente (o loop atual acorda a cada 30 s por causa da trava; isso sai).
- Quebrar o colar dentro do próprio app: o serviço **não** toma a posse da cópia enquanto o app dono
  está vivo (ver "Manter colável").
- Ressuscitar senha copiada de gerenciador de senhas.
- Instalar/recarregar o painel sem o OK do usuário (recarregar = 21 processos, microtravadas).

## Partes

### 1. Serviço (`--clipboard`)
- `main.rs` despacha `--clipboard` antes de qualquer coisa do libcosmic, como já faz com
  `--show-desktop`/`--monitor`. Não abre janela, não inicia iced.
- Thread principal: loop `calloop` com a conexão Wayland (`WaylandSource`), como o `run()` atual.
  `dispatch(None, …)`: sem timeout.
- D-Bus: um runtime tokio `current_thread` numa segunda thread, com o servidor zbus. Pedidos que
  precisam do Wayland (copiar) vão ao loop por um `calloop::channel`.
- Todas as mudanças do histórico passam por um único dono no processo (um `Mutex` em volta de
  `change`, ou tudo feito no loop calloop; quem constrói escolhe), para a gravação de um item novo e um
  "fixar" vindo do D-Bus não se atropelarem.
- Na partida: corta o histórico para 15 + fixados e cria as miniaturas que faltam (hoje isso é o
  `make_missing_thumbnails` no `init` de cada applet; sai dos applets).
- Uma instância só: o nome D-Bus é pedido sem fila (`DoNotQueue`); se já tiver dono, o processo sai
  com mensagem clara e código ≠ 0. A trava em arquivo deixa de existir.
- Compositor caiu / conexão fechou → o processo sai com erro e o systemd o reinicia.

### 2. Interface D-Bus
- Nome `io.github.Aenes0.CosmicCentral.Clipboard`, objeto `/io/github/Aenes0/CosmicCentral/Clipboard`,
  interface `io.github.Aenes0.CosmicCentral.Clipboard`.
- Métodos (todos gravam o arquivo **antes** de responder):
  - `Copy(id: t)`: põe o item na área de transferência (marcado `x-central-history`) e o leva ao topo;
  - `Pin(id: t)`: alterna fixado;
  - `Remove(id: t)`: apaga (e a imagem e a miniatura);
  - `Clear()`: apaga todos os não fixados.
  - Id inexistente: não faz nada e responde normalmente (o painel só estava desatualizado).
- Sinal `Changed()`: emitido depois de cada gravação (item novo ou qualquer método acima).

### 3. Applets (`App`)
- Saem: a conexão Wayland própria, a trava, `clipboard_tx`, `Event::Ready`, `Request`, as chamadas
  diretas a `touch`/`pin`/`remove`/`clear` e `make_missing_thumbnails` no `init`.
- Leitura: `load()` do arquivo ao abrir o popup (como hoje) e a cada sinal `Changed` (substitui o
  evento `History` que só o applet "dono da trava" recebia; agora os 3 atualizam ao vivo com o popup
  aberto, de graça: o sinal só chega quando algo muda).
- Ações: `CopyEntry/PinEntry/DeleteEntry/ClearClipboard` viram chamadas D-Bus assíncronas
  (`Task::future`). A resposta, ou o `Changed`, recarrega o arquivo. Falha na chamada: `tracing::warn!`
  e nada muda na tela (sem cair no applet).

### 4. Manter colável
Ao chegar uma cópia nova que não é nossa:
1. Se oferece `x-kde-passwordManagerHint` (gerenciadores de senha marcam assim): **não grava no
   histórico e não guarda**; esvazia o que estava guardado. *(Decisão minha: hoje uma senha copiada do
   KeePassXC entra no histórico em disco. Mudança de comportamento, de propósito.)*
2. Senão, lê **todos** os formatos oferecidos (menos os metatipos do X11: `TARGETS`, `MULTIPLE`,
   `TIMESTAMP`, `SAVE_TARGETS`, `DELETE`, `INSERT_PROPERTY`, `INSERT_SELECTION`), em paralelo, num leitor
   só com `poll()` sobre os pipes (nunca bloqueia num pipe enquanto o app escreve noutro), com prazo
   **de silêncio** (2 s sem nenhum byte em nenhum pipe; *decisão minha*: um prazo total cortaria PNGs
   grandes que o app leva segundos para gerar, e hoje não há prazo nenhum) e cortes de tamanho (16 MB
   por formato, como hoje; total de 32 MB; formato que passa do corte é descartado, os outros ficam).
   O formato que vai para o histórico tem prioridade: ao estourar o total, descarta-se primeiro os
   outros, nunca ele.
3. Guarda o conjunto (formato → bytes) na memória como "a última cópia". Do mesmo conjunto, grava no
   histórico como hoje: o primeiro tipo de texto da lista atual, senão `image/png`.

Quando o compositor avisa que a área de transferência ficou vazia (`selection` sem oferta: o app dono
fechou ou limpou), o serviço cria uma fonte sua com os formatos guardados (mais `x-central-history`) e
a põe na área de transferência. Assim o Ctrl+V continua funcionando, e o colar dentro do app enquanto
ele está aberto não muda (o app continua dono). Confirmado no smithay (base do cosmic-comp): ao destruir
a fonte do app, o compositor zera a seleção e avisa os dispositivos.

Detalhes:
- Uma fonte nossa na área de transferência não vira item novo (marca `x-central-history`, como hoje).
  "Copiar de novo" (`Copy(id)`) **substitui** o guardado pelo item escolhido (*decisão minha*: o que
  volta depois de a área de transferência ficar vazia é o último que o usuário pôs lá). A reposição não
  muda o guardado.
- Oferta sem nenhum formato legível: não substitui o guardado.
- "Copiar de novo" passa a viver no serviço: o item continua colável mesmo quando o painel recarrega.
- App que limpa a área de transferência de propósito (sem a marca de senha): o serviço repõe. É o que o
  Windows faz; aceito.
- O guardado fica só na memória. Serviço reiniciado = nada guardado até a próxima cópia.

### 5. Instalação
- `data/central-clipboard.service` (unit `systemd --user`):
  `ExecStart=@bindir@/cosmic-ext-applet-central --clipboard`, `Type=dbus`,
  `BusName=io.github.Aenes0.CosmicCentral.Clipboard`, `After=`/`PartOf=graphical-session.target`,
  `Restart=on-failure`, `WantedBy=graphical-session.target` (sobe no login, antes de o painel abrir).
- `data/io.github.Aenes0.CosmicCentral.Clipboard.service` (ativação D-Bus) com
  `SystemdService=central-clipboard.service`: um pedido com o serviço parado o sobe.
- `install.sh` instala as duas em `$prefix/share/systemd/user/` e `$prefix/share/dbus-1/services/`
  (caminhos válidos tanto em `~/.local` quanto em `/usr`). Só com o prefixo padrão (`~/.local`) e fora
  do Flatpak: `systemctl --user daemon-reload`, `enable` e `restart` do serviço (um binário novo entra
  em uso).
- Rodar o `install.sh`/`.dev/recarregar.sh` de verdade **só com o OK do usuário**.

## Organização do código
`src/clipboard.rs` (826 linhas) vira um módulo:
- `src/clipboard/mod.rs`: `Entry`, arquivo, `change`/`prune`/`record`/`touch`/`pin`/`remove`/`clear`,
  miniaturas, `preview` (compartilhado);
- `src/clipboard/service.rs`: Wayland + D-Bus + manter colável;
- `src/clipboard/panel.rs`: o painel (código visual de hoje) e o lado cliente (proxy, assinatura do
  `Changed`).
O código puro do "manter colável" (filtrar formatos, marca de senha, escolher o que gravar, ler pipes com
prazo e cortes) fica em funções testáveis sem compositor.

## Testes (TDD)
- Os testes atuais de `clipboard.rs` continuam passando.
- Puro: metatipos do X11 descartados; marca de senha detectada; escolha do formato a gravar (texto na
  ordem atual, senão PNG, senão nada); "o mesmo texto de novo só sobe e mantém o fixado".
- Leitor de pipes: vários pipes escritos fora de ordem não travam; formato acima do corte descartado sem
  derrubar os outros; total acima do corte; prazo estourado devolve o que leu.
- `change` serializado: duas mudanças concorrentes (threads) não perdem nenhuma.
- D-Bus: num bus de sessão privado (`dbus-daemon --session --print-address` do host, se existir), os
  métodos gravam o arquivo e emitem `Changed`; se não der para subir um bus nos testes, testar a lógica
  dos métodos sem o bus e verificar o bus à mão.
- Verificação manual no sistema (depois do OK para instalar): copiar texto/imagem aparece nos 3
  monitores; copiar de novo; fixar/apagar/limpar; fechar o app de onde copiou e colar; recarregar o
  painel e colar o que foi copiado de novo; `voluntary_ctxt_switches` do processo parado por 60 s sem
  uso; RSS do serviço.

## Working tree compartilhado
O trabalho é **direto no `main`** (escolha do usuário), por cima de mudanças não commitadas de outras
frentes (bandeja `src/tray/`, `--tray` em `main.rs`, `install.sh`, `Cargo.toml`, `show_desktop.rs`).
Em `main.rs` e `install.sh` só entram edições pontuais (uma linha de despacho; o bloco das units),
relendo o arquivo antes de cada edição; nada de reescrever arquivos alheios. Ninguém commita.

## Deixado para quem constrói
- Nomes internos, divisão exata entre threads (contanto que o repouso seja sem acordar).
- Se o leitor com `poll()` usa `libc` direto ou outra API já disponível.
- Mensagens de log.

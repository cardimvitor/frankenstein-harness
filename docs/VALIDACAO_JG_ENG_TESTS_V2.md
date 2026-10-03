# Prompt de execução V2: validação de harnesses com o Frankenstein V2 (jg-eng-tests e DeepSWE)

Cole este documento inteiro numa sessão do Claude Code com terminal na máquina da GPU. Execute, não apenas descreva. Não invente resultados. **Nenhum teste é pulado** (decisão 14): o que for fisicamente impossível de medir fica registrado com a evidência da tentativa, e você avisa o dono; nunca se corta em silêncio nem por tempo.

Esta versão substitui a primeira (`docs/VALIDACAO_JG_ENG_TESTS.md`) e incorpora as decisões abaixo.

## Decisões do dono do projeto (valem acima de qualquer outra regra deste documento)
1. **Só vale o resultado final.** Se o `fh` errou duas vezes e acertou na terceira, antes de entregar, conta como certo, sem penalidade. Tentativas intermediárias, rodadas de verificação e autocorreções são o processo interno do harness e não entram na nota nem no relatório como erro. Corrija o que ele deixar no disco ao terminar, sem `--keep`. Se ele desfizer a própria mudança, a entrega é o estado original. O mesmo vale para todas as configurações.
2. **O `fh` aprende, e a memória é medida em três configurações.** `fh-nomem` cria e melhora skills mas **não as usa**; `fh-mem1` cria, melhora e usa, com um agente por tarefa; `fh-memmax` cria, melhora e usa, com paralelização máxima dentro de cada tarefa e um agente que consolida tudo (seção 5.1). Cada configuração tem a sua própria memória, vazia no início; nenhuma passa memória para outra.
3. **Resolver antes de rodar e verificar.** Tudo que pode quebrar por ambiente ou configuração é resolvido e verificado antes da execução que conta. A falha de verificação do `fh` que a primeira versão apontava (não conhecer `scripts/test.sh`) já foi corrigida no próprio `fh` (seção 5.1).
4. **Concorrência em degraus de 5 em 5, medida na calibração.** O número **total** de tarefas simultâneas, somando todas as configurações que rodam juntas, começa em 5 e sobe de 5 em 5 (5, 10, 15, 20, ...). Só sobe um degrau depois de confirmar que o anterior aguentou, e **para** quando o tok/s total do servidor deixa de crescer (menos de 5% de ganho) ou cai, ou quando o tok/s por tarefa fica abaixo de 30 (o mínimo). O último degrau bom vira o teto N_MAX. A execução principal começa direto em N_MAX, já validado degrau a degrau pela calibração e pela checagem de mistura, para não repetir a rampa (seção 5.0a).
5. **Teste final de capacidade, só do `fh`.** Ao final de tudo, com a máquina só para ele, começa com 20 sessões simultâneas e vai aumentando até onde o servidor aguenta. Mede o tok/s de cada atividade para calcular o tok/s médio por usuário e descobre a capacidade máxima em uso extremo, com outras técnicas de carga além da carga real (seção 9).
6. **Quem julga a assertividade final é a LLM que conduz os testes, não o Qwen.** O juiz é você, a LLM operadora que está executando este prompt. O Frankenstein V2 (Qwen) aparece só como o modelo avaliado, dentro das configurações. Nada do que ele diz sobre o próprio trabalho entra na nota: nem o veredito interno do `fh`, nem o revisor LLM do `fh`, nem o "terminei, todos os testes passam" de qualquer configuração. Esses sinais não entram no relatório como métrica. Detalhes na seção 5.0b.
7. **Todas as configurações rodam em paralelo, isoladas, em uma ordem fixa: primeiro tudo sem harness, depois o `fh`, depois os outros harnesses.** Cada configuração roda no seu contêiner, com o seu estado, a sua chave de API e a sua cota (seção 5.0). A ordem vale para a fila de tarefas e para todo relatório: (1) `mimo9b-direto` (MiMo-9B sozinho), (2) `qwen27b-direto` (Qwen3.8-27B sozinho), (3) `qwen27b-mimo9b-puro` (Qwen3.8-27B + MiMo-9B juntos, sem harness); (4) as configurações do `fh`: `fh-nomem`, `fh-mem1`, `fh-memmax` e `qwen27b-mimo9b-fh`; (5) `opencode`, `qwen-code`, `claude-code`, `deepseek`.
8. **Modelo fixo: `nvidia/Qwen3.8-27B-NVFP4` com MTP = 3.** É o checkpoint que o dono já validou antes numa RTX PRO 6000 Blackwell. Não há comparação de checkpoints nem varredura de MTP: baixe esse modelo, suba o vLLM com MTP 3 e use-o em tudo (seção 2.0). O que muda em relação à configuração validada é só o que a máquina exigir para subir, registrado.
9. **A máquina inteira é da avaliação.** Antes de qualquer teste, libere toda a VRAM e os demais recursos parando os processos que não fazem parte da avaliação, e baixe e instale tudo de uma vez, em paralelo (seção 1, itens 7 e 8).
10. **As onze configurações comparadas, em cadeia.** `qwen27b-direto` (o modelo sozinho, sem harness: a linha de base) → `fh-nomem` (o harness, sem usar memória) → `fh-mem1` (com memória, um agente) → `fh-memmax` (com memória, paralelização máxima e consolidação), mais `opencode`, `qwen-code`, `claude-code` e `deepseek`, e as três configurações com o MiMo-9B da decisão 15 (`mimo9b-direto`, `qwen27b-mimo9b-puro` e `qwen27b-mimo9b-fh`). A cadeia isola o efeito de cada peça: o harness (`fh-nomem` contra `qwen27b-direto`), a memória (`fh-mem1` contra `fh-nomem`) e a paralelização (`fh-memmax` contra `fh-mem1`). O relatório mostra, para cada harness, se ele melhorou ou piorou o modelo em relação à linha de base.
11. **Contêineres: Docker se houver; senão Podman ou equivalente.** Detecte o que a máquina tem e use o mesmo runtime para tudo (seção 1, item 9). Sem nenhum runtime de contêiner, instale um; se não for possível, **pare e pergunte ao dono** (os benchmarks públicos não rodam sem contêineres, e o jg-eng-tests só rodaria com isolamento por usuário do sistema). Isso não vira corte: é um pedido de ajuda.
12. **Benchmark público: só o DeepSWE v1.1.** Terminal-Bench 4.0, CWE-bench v1, Vibe Code Bench e FrontierSWE v2 saíram do plano: não rode, não baixe, não escreva adaptador.
13. **Linha de base sem harness.** `qwen27b-direto` chama o modelo diretamente, uma vez por tarefa, sem ferramentas, laço de agente, verificação nem skills (`fh direct`, seção 5.2). Ela roda em todos os conjuntos de tarefas, como as outras configurações.
14. **Nenhum teste é pulado.** Todos os 130 exercícios do jg-eng-tests, todas as 113 tarefas do DeepSWE v1.1, as onze configurações em cada uma, e todas as técnicas e variantes da fase 8. **Tempo não é motivo para cortar**: se parecer longo, aumente o paralelismo dentro das guardas, retome pelo `state.json` e informe o dono do tempo restante; só ele decide cortar algo. Uma tarefa que se mostre fisicamente inviável neste hardware **é executada mesmo assim**: o log da falha é a evidência, o resultado fica como "inviável neste hardware" (igual para as onze configurações) e o dono é avisado sem a execução parar.
15. **Três configurações com o MiMo-V2.6-Distill-Qwen-9B, sempre nomeadas assim nos relatórios.** (a) **Qwen3.8-27B + MiMo-9B, em duas versões sempre nomeadas assim**: o Qwen3.8-27B puro é o planejador, orquestrador, consolidador e verificador; o **MiMo-V2.6-Distill-Qwen-9B** é o operário que escreve o código. Fluxo de mão única: o Qwen3.8-27B lê o repositório e escreve a ordem de trabalho, o MiMo-9B escreve o código **uma vez** e devolve, e o Qwen3.8-27B verifica e corrige sozinho, **sem nunca devolver nada ao MiMo-9B**. `qwen27b-mimo9b-puro` é **sem harness**: a verificação do Qwen3.8-27B é uma única requisição, sem ferramentas (seção 5.9), e fica no bloco "tudo sem harness"; `qwen27b-mimo9b-fh` verifica com o `fh` completo (seção 5.7), no bloco do `fh`. É um teste para saber se um operário barato traz ganho de desempenho. (b) **`mimo9b-direto`**: o MiMo-9B sozinho, sem harness (`fh direct`), em todos os conjuntos, como o `qwen27b-direto` do Qwen3.8-27B. O MiMo-9B roda com **a melhor configuração de MTP, escolhida isoladamente por velocidade e assertividade** antes de tudo (seção 2.2), numa varredura que roda **em paralelo** com a preparação e a calibração do Qwen3.8-27B. Isso é uma exceção explícita à decisão 8 (modelo fixo) e à regra 2 (um modelo só): o Qwen3.8-27B continua sendo o único modelo das outras oito configurações, e o MiMo-9B só é alcançável pelas três configurações de que faz parte (seção 3 item 9).

Modelos (nomes diretos): **Qwen3.8-27B** = https://huggingface.co/nvidia/Qwen3.8-27B-NVFP4 (revisão `dbb8f445`, servido como `frankenstein-v2`); **MiMo-9B** = https://huggingface.co/XiaomiMiMo/MiMo-V2.6-Distill-Qwen-9B (oficial, BF16, sem pesos de MTP) e a versão com MTP https://huggingface.co/ycui7/MiMo-V2.6-Distill-Qwen-9B-NVFP4-MTP (servido como `mimo9b`). Ids das configurações com eles: `mimo9b-direto`, `qwen27b-direto`, `qwen27b-mimo9b-puro` (os dois juntos, sem harness) e `qwen27b-mimo9b-fh` (os dois juntos, com o `fh`).

Nomes usados aqui: **Frankenstein V2** é o modelo (Qwen3.8-27B NVFP4 + MTP=3 no vLLM). **Frankenstein Harness** (`fh`) é o nosso harness. **Configuração** é uma linha da lista do item 10 (um harness num modo). Não confunda o modelo com o harness no relatório.

## CONFIGURAÇÃO (preencher antes de colar)
- REPO: https://github.com/dfnb/jg-eng-tests, fixado no commit `36cbe4741e5730fae876fae3bff6fa167fb18cb7`. Se o HEAD for outro, pare e avise.
- GPU: uma RTX PRO 6000 Blackwell 96 GB (SM120), inteira para a avaliação (decisão 9). Modelo fixo: `nvidia/Qwen3.8-27B-NVFP4`, MTP 3 (seção 2.0)
- WORKDIR: `<ex.: /workspace/harness-eval>` (precisa de ~250 GB livres no mínimo, 400 a 500 GB recomendados; veja a seção 1.1: as onze configurações têm cópias próprias de cada exercício e de cada tarefa, mais ~30 GB para o checkpoint NVFP4 e as imagens dos benchmarks)
- FRANKENSTEIN_HARNESS: repositório https://github.com/cardimvitor/frankenstein-harness, branch `ccr-3bc51f62-prkdq0`. Compilar com `cargo build --release` e usar `target/release/fh`. Registre o commit.
- TAREFAS_SIMULTANEAS_TOTAIS (G): rampa de 5 em 5 (5, 10, 15, 20, ...) na calibração, até o tok/s total parar de crescer (menos de 5% de ganho) ou cair, ou o tok/s por tarefa ficar abaixo de 30. É o total de tarefas rodando ao mesmo tempo somando as onze configurações; o teto N_MAX sai da calibração e a execução principal começa nele (seção 5.0a)
- TIMEOUT_POR_EXERCICIO: 900 s de parede, contando do início da configuração até o processo terminar
- REPETICOES: 1 (3 se houver tempo, só para as 2 melhores configurações)
- AMOSTRAGEM: temperatura 1,0, top_p 0,95, top_k 20, igual para todos e imposta pelo proxy (seção 3)
- EXECUTAR_ATE: fase `<0 a 8>` (a 7 são os benchmarks públicos; a 8, o teste de capacidade do `fh`, sempre por último)

Estimativa de tempo, para planejar. **Nenhum teste é pulado**, então o relógio é o que ele for; o que se controla é o paralelismo. jg-eng-tests: 11 configurações × 130 exercícios = 1 430 execuções, ~7 h se N_MAX chegar a 20 e ~14 h se parar em 10 (pior caso, tudo estourando os 900 s: ~13 h e ~26 h). DeepSWE v1.1: 113 tarefas × 11 = 1 243 execuções, com timeout oficial de 3 h por tarefa: de 1 a 3 dias, conforme o paralelismo que o host aguentar. Some a calibração (~1,5 h), o passe de tempo (~4 h) e a fase 8 (~4 a 6 h). O orquestrador precisa retomar pelo `state.json` depois de qualquer queda, e o operador informa o dono do tempo restante estimado ao fim de cada fase, para que **só o dono** decida se quer mudar algo.

## 0. Regras que valem a sessão inteira
1. As configurações avaliadas só veem a pasta `exercise/` de cada desafio. Nunca `solution/`, `grader/`, `EVALUATION.md`, o `.git` do jg-eng-tests, as pastas de outros exercícios ou resultados de outras configurações. A única exceção é a memória interna do próprio `fh` (decisão 2): o que ele aprendeu sozinho nos exercícios anteriores da **sua** configuração, nunca a nota do grader. Você (operador) nunca resolve exercício, nunca dá dica e nunca cola conteúdo do grader em prompt.
2. Dois modelos locais, nada externo: o Frankenstein V2 (Qwen3.8-27B) servido pelo nosso vLLM, que é o único modelo das outras oito configurações, e o MiMo-V2.6-Distill-Qwen-9B, servido por um segundo vLLM local e alcançável **só** por `mimo9b-direto`, `qwen27b-mimo9b-puro` e `qwen27b-mimo9b-fh` (decisão 15). Nenhuma configuração pode chamar modelo externo. Bloqueie a saída de rede dos agentes (só o proxy local é alcançável) e prove pelos logs do proxy e do firewall que 100% das chamadas de modelo foram para o vLLM local.
3. Mesmo prompt de tarefa para todas as configurações, mesmo timeout, mesma amostragem, mesma ordem de exercícios (a do `catalog.json`) e o mesmo orçamento de tarefas simultâneas, repartido de forma justa (seção 5.0a).
4. **Um vLLM recém-iniciado por fase** (jg-eng-tests e DeepSWE), compartilhado por todas as configurações que rodam juntas nessa fase, com cache vazio no início. Como elas dividem a GPU, o tempo, o TTFT e o tok/s de cada uma ficam contaminados pela carga das outras: **as comparações de velocidade vêm do passe de tempo da seção 5.0c**, com cada configuração sozinha no servidor. Qualidade e tokens não dependem disso. A memória do `fh` começa vazia no início de cada configuração do `fh`.
5. Não mude a configuração do servidor entre fases nem entre configurações. Se algo precisar mudar, rode como variante separada, com nome próprio, fora da comparação principal.
6. Não grave chaves ou tokens em arquivo, log ou relatório. Nos logs, identifique chaves pelo SHA-256 truncado.
7. Não altere nenhum arquivo do jg-eng-tests, nem os graders. Correções sugeridas vão só para o relatório.
8. O julgamento final (seção 5.0b) é feito só pela LLM operadora, com o grader oficial como base objetiva. É proibido usar o vLLM/Qwen, ou qualquer configuração avaliada, para julgar, resumir ou classificar resultados. O operador também não usa o julgamento para ajudar os agentes: ele só acontece depois que o exercício terminou.
9. Um erro ou resultado estranho do nosso harness (`fh`) é um achado, não algo a esconder. Registre com evidência. Problemas encontrados *antes* da execução que conta (fase 0, pilotos) são corrigidos antes de começar (decisão 3): registre o commit do `fh` usado. Não corrija o `fh` no meio da comparação, porque isso invalidaria a execução. Se a correção for indispensável, termine a execução, corrija, e rode de novo como variante com nome próprio.
10. **Isolamento entre configurações que rodam juntas.** Nada é compartilhado entre elas além do vLLM, do proxy (que só mede e limita) e de caches de pacotes em modo de leitura: cada uma tem o seu contêiner por tarefa, a sua rede interna, os seus diretórios de trabalho e de estado, a sua chave de API e as suas cotas de CPU e memória (seção 5.0). Uma configuração nunca lê arquivos, logs ou memória de outra.

## 1. Fase 0: preparação e sanidade (antes de qualquer configuração)
1. Clone o jg-eng-tests em `WORKDIR/src/jg-eng-tests` e confira o commit. Rode:
   - `python3 scripts/validate-structure.py`
   - `python3 scripts/run-graders.py`: todas as 130 referências devem passar no próprio grader. Uma referência que não passar vira "defeituoso" na avaliação independente e sai do denominador principal, listada.
   - `python3 scripts/audit-initial-states.py`: nenhum exercício pode começar resolvido.

   Registre as saídas.
2. Fatos do repositório, para o relatório:
   - 130 exercícios.
   - Stacks: 70 só .NET 10, 5 .NET combinado com outra coisa (linq, sql, bash+powershell, docker-compose, typescript+yaml), 29 React+TypeScript e 26 Angular+TypeScript.
   - Níveis: 7 beginner, 79 intermediate, 44 advanced.
   - Trilhas: as 13 do `catalog.json`.
   - 70 exercícios têm `<unimplemented>` no código inicial. Gere essa lista com `grep -rl "<unimplemented>" exercises/*/exercise` e use-a na análise por tipo.
   - Todos têm `scripts/setup.sh`, `test.sh`, `lint.sh` e `start.sh`.
   - `global.json` pede o SDK `10.0.100` com `rollForward: latestFeature`, então 10.0.401 serve. `.node-version` = `24.15.0`.
   - O exercício 29 (`reproducible-local-env`) precisa de Docker. O que usa bash+powershell precisa de `pwsh`.

   Confira esses números por script; se diferirem, use os medidos.
3. Imagem de execução única, a mesma para todas as configurações (Docker ou Podman, seção 1 item 9). Deve conter:
   - .NET SDK 10.0.401, Node 24.15.0 (a do `.node-version`), `pwsh`, git, python3.
   - Os CLIs de todos os harnesses em versões fixas.
   - Os language servers que o `fh` usa quando existem: `csharp-ls`, `typescript-language-server` + `typescript`, `pyright`, `rust-analyzer`. São parte do harness; registre as versões.

   Registre o digest da imagem. Para o exercício 29, os agentes não têm Docker dentro do contêiner, e isso vale igual para todos. Marque-o como "limitado pelo ambiente" e reporte o resultado com e sem ele.
4. Ambiente do grader: o mesmo .NET 10.0.401 e Node 24.15.0, fora do contêiner dos agentes, numa pasta que só o operador acessa. Registre as versões. Os relatórios anteriores do Claude usaram .NET 10.0.112; anote a diferença.
5. **Ensaio geral sem modelo (decisão 3). Bloqueia a fase se algo falhar.** Para os 130 exercícios, dentro da imagem de execução:
   1. Copie `exercise/` e rode `setup.sh` com rede.
   2. Depois, **sem rede**, rode `test.sh` e `lint.sh` no estado inicial. Os dois precisam executar até o fim, sem erro de ambiente: pacote faltando, SDK errado, restore tentando ir à rede, permissão. Falhar nos testes é o esperado, porque o exercício ainda não foi feito.
   3. Rode o grader sobre essa cópia inicial: ele precisa rodar e reprovar.
   4. Copie a `solution/` por cima de uma cópia descartável (só o operador faz isso, fora de qualquer pasta de agente) e confira que `test.sh` e `lint.sh` passam, sem rede.

   Registre a tabela exercício × (setup, test inicial, lint inicial, grader inicial, test e lint com a referência). Cada falha de ambiente é corrigida na imagem ou no procedimento antes de continuar; nunca nos arquivos do exercício. Um exercício que não pode ser consertado assim sai do denominador, listado com o motivo.
6. **Verificação do `fh` antes de rodar.** Compile o `fh` no commit escolhido e rode `cargo test --release`: tudo precisa passar. Rode `fh doctor` contra o proxy. Nos pilotos, confirme no JSON de `--json` (`verify.rounds[].checks[].name`) que o `fh` verificou com `scripts/lint` e `scripts/test`, e não com `dotnet test` ou `npm test` inferidos.

7. **Máquina dedicada: libere todos os recursos (decisão 9).** Esta GPU e esta máquina são só da avaliação. Faça isto antes de baixar ou instalar qualquer coisa que dependa de recurso.
   1. **Inventário antes de mexer.** Grave em `WORKDIR/maquina/antes.txt`: `nvidia-smi`, `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv`, `ps aux --sort=-%mem | head -40`, `ps aux --sort=-%cpu | head -40`, `systemctl list-units --type=service --state=running`, `docker ps -a` (ou `podman ps -a`), `df -h`, `free -h`, `lscpu`, `who`, `uptime`.
   2. **Libere a GPU.** Pare tudo que usa a placa: vLLM ou Ollama antigos, notebooks, treinos, contêineres com `--gpus`, outros servidores de modelo. Primeiro do jeito educado (`systemctl stop`, `docker stop`, `SIGTERM` e espera de 10 s), depois `SIGKILL` no que sobrar. Impeça que voltem durante a sessão (`systemctl disable --now`, `docker update --restart=no <contêiner>`, ou `systemctl mask` temporário). Alvo: `nvidia-smi` sem nenhum processo de computação e com menos de 500 MiB de VRAM usada. Se a VRAM continuar ocupada sem processo dono, tente `fuser -v /dev/nvidia*` e `nvidia-smi --gpu-reset`; se ainda assim não liberar, pare e avise o dono (pode exigir reiniciar a máquina).
   3. **Libere CPU, RAM e disco.** Pare os processos que consomem CPU ou RAM sem fazer parte da avaliação. Libere disco com `docker container prune`, `docker builder prune` e `docker image prune` (só imagens sem uso e sem nome). Não apague volumes, bancos de dados nem arquivos de pessoas.
   4. **Nunca mate, em nenhuma hipótese:** a sua própria sessão e o shell em que você roda (inclusive `tmux`/`screen`), o `sshd` e as conexões SSH ativas, `systemd`/`init`, `dockerd`/`containerd`, a rede (`NetworkManager`, `systemd-networkd`, resolvedor de nomes), o driver da NVIDIA e o `nvidia-persistenced`.
   5. **Pergunte antes de parar:** processo de outro usuário com sessão interativa ativa, e qualquer serviço que pareça de produção ou guarde dados de outras pessoas (banco de dados, servidor web, fila). Se for ambíguo, pergunte; não presuma.
   6. **Registre tudo.** `WORKDIR/maquina/parado.md`: o que foi parado, o dono, o comando e o comando para restaurar. Ao final da sessão, ofereça restaurar.
   7. **Desempenho (precisa de root; sem root, anote que não deu):** `nvidia-smi -pm 1` (modo persistente), governador de CPU em `performance` (`cpupower frequency-set -g performance`), `ulimit -n` alto, `/dev/shm` com pelo menos 16 GB (e `--shm-size` nos contêineres) e swappiness baixo. Grave os valores em `WORKDIR/maquina/depois.txt`, junto de um segundo `nvidia-smi` mostrando a VRAM livre.
   8. **Reserva.** O vLLM e o proxy têm prioridade. Deixe sempre livres pelo menos 4 núcleos e 16 GB de RAM para eles, e limite o paralelismo das tarefas de acordo (as `cpus` e `memory_mb` de cada tarefa são o teto; reduza `-n` antes de tirar recurso do vLLM).
8. **Baixe e instale tudo logo no começo, em paralelo.** Primeiro confira o espaço em disco contra a soma do que vai baixar (checkpoints, imagens Docker, datasets, caches); se faltar, pare e avise. Depois dispare, em paralelo e em segundo plano, cada item com seu log em `WORKDIR/instalacao/<item>.log`, com no máximo 4 a 6 downloads simultâneos, para não saturar disco e rede:
   - o checkpoint `nvidia/Qwen3.8-27B-NVFP4` (`hf download --revision dbb8f445`, com `HF_HUB_ENABLE_HF_TRANSFER=1`);
   - **os checkpoints do MiMo-9B, só os da seção 2.2** (decisão 15): `ycui7/MiMo-V2.6-Distill-Qwen-9B-NVFP4-MTP` em `WORKDIR/modelos/mimo9b-nvfp4-mtp` e o controle `XiaomiMiMo/MiMo-V2.6-Distill-Qwen-9B` (BF16) em `WORKDIR/modelos/mimo9b-bf16`, cada um com `hf download <repo> --local-dir <pasta>` e a revisão registrada. Comece por eles, para a varredura de MTP da seção 2.2 começar logo. Nenhum outro checkpoint, além dos que a seção 2.2 mandar procurar;
   - vLLM em versão fixa, ou a sua imagem Docker, e o driver/CUDA que a receita pede;
   - imagens de contêiner: as bases dos exercícios, as do DeepSWE, as do proxy e do LiteLLM;
   - toolchains: Rust (e o alvo musl do `fh`), .NET SDK 10.0.401, Node 24.15.0, `pwsh`, Python 3.12 com `uv`, e os language servers;
   - os harnesses: OpenCode, Qwen Code, Claude Code, DeepSeek Harness, LiteLLM (o `qwen27b-direto` usa o próprio `fh direct`, sem nada a instalar);
   - Harbor e Pier em versões fixas, e os datasets e repositórios: `jg-eng-tests`, `datacurve-ai/deep-swe`;
   - os caches de pacotes dos exercícios (`dotnet restore`, `npm ci`), já que depois os agentes rodam sem rede.

   Compile o `fh` (nativo e estático) enquanto os downloads rodam. Ao terminar, verifique tudo (checksums dos checkpoints, `--version` de cada ferramenta) e grave `WORKDIR/instalacao/VERSOES.md`. O que falhou é repetido uma vez; se falhar de novo, vai para o `REVISAO.md`. Nada pode depender de internet depois desta etapa.

9. **Runtime de contêineres: Docker, senão Podman (decisão 11).** Onde este documento diz `docker`, vale o runtime que a máquina tiver.
   1. **Detecte.** Se `docker info` responde, use Docker. Senão, se existe `podman`, use Podman. Se não existe nenhum, tente instalar (`apt-get install podman` ou o pacote da distribuição) quando houver root. Sem root e sem runtime, pare e pergunte ao dono. Enquanto ele não responde, o jg-eng-tests pode começar com isolamento por usuário do sistema (um usuário por configuração, com `bubblewrap` quando houver), mas os benchmarks públicos continuam PENDENTES até haver um runtime; nada é descartado.
   2. **Podman com Harbor e Pier.** Os dois conversam com a API do Docker (`docker` e `docker compose`). Ligue o socket compatível do Podman (`systemctl enable --now podman.socket`, ou `systemctl --user enable --now podman.socket` no modo rootless), aponte `DOCKER_HOST` para ele (`unix:///run/podman/podman.sock`, ou `unix://$XDG_RUNTIME_DIR/podman/podman.sock` no rootless), e tenha o cliente `docker` (pacote `podman-docker`) e o `docker compose` ou o `podman-compose`. Antes de seguir, prove que funciona: `docker version`, `docker compose version`, e um build e um run pequenos com uma tarefa de exemplo do Harbor e do Pier (o Pier sobe um Squid como proxy de saída no próprio compose: teste isso também).
   3. **Rootless.** Precisa de espaço em `~/.local/share/containers`. Portas abaixo de 1024 exigem `sysctl net.ipv4.ip_unprivileged_port_start=80`, e o DeepSWE precisa da porta 80 (seção 8.2): sem root para isso, tente um encaminhador de porta 80 dentro de um contêiner (no modo rootless o contêiner pode escutar em portas baixas no próprio namespace), confirme no piloto do DeepSWE que o `fh` recebe resposta e, se não funcionar, **pare e peça ao dono** o `sysctl` (ação de root); o DeepSWE fica PENDENTE até lá, nunca descartado. Se os limites de CPU e memória por tarefa não funcionarem (cgroups v2 não delegados), anote: a cota por configuração da seção 5.0 passa a ser só do proxy e do orçamento de tarefas.
   4. **Rede interna e GPU.** `docker network create --internal` vira `podman network create --internal`; reconfira a prova de isolamento da seção 3 item 7 com o runtime escolhido. Os agentes não usam GPU, só o vLLM: se ele rodar em contêiner Podman, use o CDI (`--device nvidia.com/gpu=all`); se rodar direto no host, nada a fazer.
   5. **Registre** o runtime, a versão (`docker --version` ou `podman --version`) e se é root ou rootless no `MANIFEST.json`, e use o mesmo para tudo.

### 1.1 Orçamento de VRAM e de disco (estimativas para planejar; o preflight e o B12 medem os números reais)
**VRAM (96 GB no total).**

| Item | Pesos | KV cache e extras | Fração configurada |
|---|---|---|---|
| Qwen3.8-27B NVFP4 + MTP 3 (servidor 1) | ~16 a 18 GB | o resto do orçamento, ~50 a 55 GB (FP8, contexto 131072; o vLLM imprime a concorrência máxima na subida) | 0,78 (~75 GB) |
| MiMo-9B NVFP4 + MTP (servidor 2) | ~5 a 6 GB | ~6 a 7 GB | 0,14 (~13 GB) |
| Folga para contextos CUDA, grafos e picos | | | ~0,08 (~8 GB) |
| MiMo-9B BF16 oficial (só o controle, sozinho na GPU) | ~19 GB | ~6 a 10 GB | ~0,30 |

Os dois servidores juntos usam 0,92 da placa. Sozinho, o Qwen3.8-27B pode usar 0,90 a 0,95 (varredura do MTP, passe de tempo e fase 8 do Qwen3.8-27B, se o MiMo-9B for desligado). Se o KV do Qwen3.8-27B não der para N_MAX sessões, reduza o contexto do MiMo-9B antes de mexer no 27B.

**Disco (SSD, dentro de WORKDIR; use NVMe se possível).**

| Item | Espaço |
|---|---|
| Checkpoint Qwen3.8-27B NVFP4 | ~18 GB |
| Checkpoint MiMo-9B NVFP4 + MTP | ~8 GB |
| MiMo-9B BF16 oficial (controle) | ~19 GB |
| vLLM (ambiente ou imagem, torch, caches de compilação e FlashInfer) | ~15 a 25 GB |
| Imagens de contêiner do jg-eng-tests (.NET, Node, bases) + caches de NuGet/npm | ~15 a 25 GB |
| Imagens do DeepSWE v1.1 (113 tarefas, camadas compartilhadas) | incerto, ~40 a 120 GB: meça somando as imagens após o build |
| Ferramentas, harnesses, Harbor, Pier, Rust, .NET, Node | ~10 a 15 GB |
| Cópias dos exercícios por configuração (130 × 11) | ~15 a 40 GB **se** apagar `node_modules`, `bin`, `obj` e caches depois da correção e guardar só o diff e o JSON do grader; **mais de 300 GB** se guardar tudo |
| Logs: proxy (metadados, sem corpo das requisições), `/metrics`, saídas dos agentes | ~10 a 60 GB |
| Sessão do ai-jail (alvo de cargo, imagens com Rust, 86 tarefas) | ~15 a 30 GB |
| Margem de segurança | 20% |

**Total: ~250 GB no mínimo com a limpeza descrita; recomendado 400 a 500 GB.** Registre o espaço livre antes e depois de cada download e de cada fase. Apague as cópias dos exercícios (menos o diff e o JSON do grader) logo depois da correção; a regra vale para todas as configurações igualmente.

## 2. Fase 1: servidor, configuração validada do Frankenstein V2

### 2.0 Modelo fixo: `nvidia/Qwen3.8-27B-NVFP4`, MTP 3 (decisão 8)
O modelo da sessão inteira é o checkpoint https://huggingface.co/nvidia/Qwen3.8-27B-NVFP4, com decodificação especulativa MTP de 3 tokens (`num_speculative_tokens = 3`). O dono do projeto já o validou numa RTX PRO 6000 Blackwell, então **não há escolha de checkpoint, nem varredura de MTP, nem comparação com BF16, FP8 ou quantizações da comunidade**. Esse é o "Frankenstein V2" de todas as fases.

- **Revisão.** Use a revisão que o dono validou, `dbb8f445`, e registre o hash completo do commit e o checksum dos arquivos baixados. Se essa revisão não existir mais ou não baixar, **pare e pergunte** antes de usar outra; não troque de revisão em silêncio.
- **Comando.** O da seção 2.1, que é a configuração validada, com as flags exatamente como estão (`--quantization nvfp4`, `--speculative-config '{"method":"qwen3_5_mtp","num_speculative_tokens":3}'`, parsers, FP8 no KV cache, prefix caching). Confira cada flag e o nome do método MTP com `vllm serve --help` da versão instalada e com o model card, e use a sintaxe que essa versão aceitar, mantendo o mesmo significado.
- **O que pode mudar, e só se for preciso.** Para o servidor subir ou aproveitar a GPU livre:
  - `--gpu-memory-utilization` entre 0,90 e 0,95, já que a GPU não tem outros processos desde a fase 0 (se faltar memória, desça);
  - `--max-model-len` (131072, ou o maior valor que ainda deixe KV cache para N_MAX sessões);
  - `--max-num-seqs` conforme a calibração da seção 5.0a;
  - a correção do SM120 (`--quantization modelopt_fp4 --block-size 128`) só se o comando padrão travar na inicialização.

  Nada disso muda o modelo nem o MTP. Registre cada ajuste e o motivo no `MANIFEST.json`.
- **Verificação de que é o mesmo modelo validado (bloqueia a fase 1).** Depois de subir, rode uma sessão de geração com prompts de agente (entrada de 20 a 60 mil tokens, com prefixo compartilhado, saída de 500 a 2000) e compare com a referência do dono: **122,1 tok/s por fluxo** e **aceitação do draft de 75,7%**, lida do `/metrics`. Aceite uma variação de até 15% em cada uma. Se ficar fora disso, investigue o ambiente (versão do vLLM e do CUDA, driver, `VLLM_HAS_FLASHINFER_CUBIN`, clock da GPU, processos ainda na GPU) e corrija o ambiente; **não** troque o modelo nem o MTP. Se não resolver, pare e pergunte.
- **Fixo para sempre.** Depois dessa verificação, o servidor só reinicia com o mesmo comando (regras 4 e 5). Qualquer outra configuração é variante com nome próprio (fase 8).

### 2.1 Configuração de referência (linha de base)
Base: Qwen3.8-27B em NVFP4 (checkpoint `nvidia/Qwen3.8-27B-NVFP4`, revisão `dbb8f445`) com MTP=3, vLLM 0.29.0 ou superior, CUDA 13.x. Resultado de referência nessa configuração: 122,1 tok/s por fluxo (52,2 sem MTP) e aceitação do draft de 75,7%.

    VLLM_HAS_FLASHINFER_CUBIN=1 vllm serve nvidia/Qwen3.8-27B-NVFP4 \
      --revision dbb8f445 \
      --served-model-name frankenstein-v2 \
      --quantization nvfp4 \
      --speculative-config '{"method":"qwen3_5_mtp","num_speculative_tokens":3}' \
      --reasoning-parser qwen3 \
      --enable-auto-tool-choice --tool-call-parser qwen3_xml \
      --kv-cache-dtype fp8 \
      --enable-prefix-caching \
      --max-model-len 131072 \
      --max-num-seqs 64 \
      --gpu-memory-utilization 0.90 \
      --override-generation-config '{"temperature":1.0,"top_p":0.95,"top_k":20}' \
      --host 127.0.0.1 --port 8000

Notas:
- `max-model-len`: 131072 se couber na GPU; mínimo 65536. Na rodada anterior, 32768 estourou num exercício, e Claude Code e OpenCode têm prompt de sistema grande. Registre o valor usado e use o mesmo valor em `FH_CONTEXT_WINDOW` (seção 5.1).
- Se o servidor travar ao iniciar com MTP no Blackwell (SM120), aplique a correção que resolveu antes: `--quantization modelopt_fp4 --block-size 128` com `VLLM_HAS_FLASHINFER_CUBIN=1`. Registre qual foi usado.
- Confira cada flag com `vllm serve --help` da versão instalada; não use sintaxe de memória. Se `--override-generation-config` não existir, use o equivalente da versão e registre. De qualquer forma, o proxy impõe a amostragem (seção 3), porque os harnesses mandam os próprios valores por requisição e o padrão do servidor não os sobrescreve.
- `max-num-seqs`: 16 foi validado na PRO 5000, mas com a rampa de 5 em 5 (que pode passar de 20 tarefas simultâneas, mais as requisições extras do `fh-memmax`, que abre scouts e workers dentro de cada tarefa), 16 vira fila na hora. Suba para 64 na PRO 6000 de 96 GB. Na calibração (seção 5.0a), confira no `/metrics` se o uso de KV cache passa de 95% ou se há preempções. Se passar, reduza para 48 ou 32 e registre. Uma vez escolhido, o valor é fixo para todas as configurações. Se a rampa parar porque o servidor encheu (fila sustentada em `num_requests_waiting` com `max-num-seqs` cheio), registre: o limite foi do servidor e não da GPU; subir `max-num-seqs` seria uma variante com nome próprio.
- Com MTP, o ganho da especulação cai quando o lote cresce: a GPU deixa de estar ociosa entre tokens. É por isso que a rampa mede tok/s e aceitação a cada degrau. Se a calibração mostrar que acima de certo N o MTP piora o agregado, registre como achado. Não desligue o MTP na comparação principal; isso é testado como variante na fase 8.
- Reinício "limpo" = matar o processo, esperar a VRAM voltar ao repouso no `nvidia-smi`, subir de novo e esperar `GET /v1/models` responder. O cache de prefixo morre com o processo; não é preciso mais nada.
- Teste de fumaça, a cada reinício (uma vez por fase) e antes de liberar as configurações:
  1. Uma chamada simples.
  2. Uma chamada com ferramenta: confira que `tool_calls` vem estruturado e que o conteúdo não traz XML cru.
  3. Uma chamada com streaming, conferindo que `reasoning_content` vem separado.
  4. Leitura de `/metrics`, conferindo que existem as métricas de decodificação especulativa (nomes com `spec_decode`).

  Use também `fh doctor` e, só na primeira vez, `fh validate-vllm` apontados para o proxy. Guarde as saídas.

### 2.2 O MiMo-V2.6-Distill-Qwen-9B: onde baixar e a melhor configuração de MTP (decisão 15)

**O que existe (pesquisado pelo autor do plano; confirme tudo no Hugging Face com `hf`/`huggingface_hub`, porque a pesquisa não conseguiu abrir as páginas e se baseou em resumos de busca).**
- `XiaomiMiMo/MiMo-V2.6-Distill-Qwen-9B`: o modelo oficial (Xiaomi MiMo, licença MIT), um SFT do Qwen3.5-9B sobre dados gerados pelo MiMo, em safetensors BF16. O checkpoint oficial **declara uma camada MTP mas não traz os pesos dela**: serve como controle de qualidade sem MTP (`num_speculative_tokens = 0`), não como candidato a MTP.
- `ycui7/MiMo-V2.6-Distill-Qwen-9B-NVFP4-MTP`: quantização NVFP4 (PTQ pelo TensorRT Model Optimizer) do oficial, com a cabeça MTP embutida em precisão cheia. O model card pede vLLM ≥ 0.30, `--kv-cache-dtype fp8` (o NVFP4 no KV cache só existe em SM100) e usa `--spec-method mtp --speculative-config '{"num_speculative_tokens": 2}'`; o autor relata, numa máquina que não é a nossa, 182,8 / 206,1 / 305,8 / 164,5 tok/s para `num_speculative_tokens` = 0 / 1 / 2 / 3. **Trate esses números como hipótese a verificar**, não como resultado. É o candidato principal por ser o único achado já pronto para o vLLM, em NVFP4 como o Qwen3.8-27B, e para Blackwell.
- Outras versões com MTP encontradas, **não utilizáveis no vLLM** e por isso fora da varredura: GGUF com cabeça MTP (`VitreousCut/…-MTP-GGUF`, `IHaveNoClueAndIMustPost/…-Q8_0-MTP`: llama.cpp) e MLX com MTP enxertado (`xunlinkx/…-oQ3e/oQ4e/oQ5e-mtp`: Apple Silicon). Variantes sem MTP declarado (FP8 de `lovedheart`, INT4 AutoRound de `letechlead`, ablitrated de `Hikari07jp`) também ficam de fora.
- **Busca obrigatória antes da varredura (item 1):** pesquise no Hub, por `MiMo-V2.6-Distill-Qwen-9B` com `mtp`, `nvfp4`, `fp8`, `bf16`, se há alguma versão com MTP que o vLLM instalado carregue além da `ycui7` (por exemplo BF16 ou FP8 com a cabeça enxertada). Cada candidato que carregar entra na varredura; registre os que foram achados e por que algum foi descartado.

**Onde baixar e usar.** Pastas locais fixas (a revisão baixada e o SHA-256 dos arquivos vão para o `MANIFEST.json`):

    hf download ycui7/MiMo-V2.6-Distill-Qwen-9B-NVFP4-MTP --local-dir $WORKDIR/modelos/mimo9b-nvfp4-mtp
    hf download XiaomiMiMo/MiMo-V2.6-Distill-Qwen-9B        --local-dir $WORKDIR/modelos/mimo9b-bf16

Servido por um **segundo vLLM**, na porta 8000 do Qwen3.8-27B + 2 (isto é, 8002 atrás do proxy e 8003 direto; ajuste se ocupadas), com nome servido `mimo9b`:

    VLLM_HAS_FLASHINFER_CUBIN=1 vllm serve $WORKDIR/modelos/mimo9b-nvfp4-mtp \
      --served-model-name mimo9b \
      --speculative-config '{"method":"mtp","num_speculative_tokens":<K>}' \
      --reasoning-parser qwen3 --enable-auto-tool-choice --tool-call-parser qwen3_xml \
      --kv-cache-dtype fp8 --enable-prefix-caching \
      --max-model-len <igual ao do Qwen3.8-27B, no mínimo 65536> --max-num-seqs 32 \
      --gpu-memory-utilization 0.14 \
      --host 127.0.0.1 --port 8003

`<K>` é o resultado da varredura. Confira cada flag e o nome do método com `vllm serve --help` e o model card da versão instalada (o Qwen3.8-27B usa `qwen3_5_mtp`; o do MiMo-9B pode usar outro nome). Se o MiMo-9B não aceitar o parser `qwen3` ou o de ferramentas, use o equivalente do model card: o MiMo-9B só escreve texto na seções 5.7 a 5.9 e nunca chama ferramentas, mas o teste de fumaça confere assim mesmo.

**Divisão da GPU (as duas instâncias convivem durante a sessão inteira).** Com os dois servidores de pé, o Qwen3.8-27B sobe com `--gpu-memory-utilization` 0,78 (em vez dos 0,90 da seção 2.1: **é o único ajuste do Qwen3.8-27B**, justificado pelo MiMo-9B) e o MiMo-9B com ~0,14, deixando ~8% de folga. A verificação da seção 2.0 (122,1 tok/s por fluxo, 75,7% de aceitação, margem de 15%) é refeita **com o servidor do MiMo-9B ligado e ocioso**. Se o Qwen3.8-27B ficar sem KV cache suficiente para N_MAX sessões, reduza o `--max-model-len` e o `--max-num-seqs` do MiMo-9B antes de mexer no Qwen3.8-27B, e registre.

**Varredura de MTP do MiMo-9B (roda em paralelo com a fase 0, a preparação e a calibração do Qwen3.8-27B, mas termina antes da execução principal).**
1. Candidatos: a `ycui7` NVFP4 com `num_speculative_tokens` = 0, 1, 2, 3, 4 (o 0 é a linha de base sem MTP), mais os candidatos achados na busca acima. Cada ponto = uma subida limpa do servidor do MiMo-9B NVFP4 (`--gpu-memory-utilization` 0,14, igual ao uso real), um por vez, na porta 8003. **O controle BF16 oficial (K = 0) não cabe em 0,14**: os pesos sozinhos ocupam ~19 GB (0,14 de 96 GB são ~13 GB). Rode-o separado, **com o Qwen3.8-27B desligado** (por exemplo logo depois do B2 do portão B, antes do primeiro start do Qwen3.8-27B, ou entre duas fases), com `--gpu-memory-utilization` ~0,30, nos mesmos 24 prompts e 20 exercícios; ele serve só de controle de qualidade da quantização NVFP4 e não de velocidade.
2. **Contaminação.** A varredura divide a GPU com o Qwen3.8-27B em preparação ou calibração, e a velocidade é afetada. Controle isso de três maneiras: (a) rode os pontos **intercalados** (K=0,1,2,3,4,0,1,2,3,4, 2 passadas) para que a interferência atinja todos por igual; (b) decida por **razões** contra o K=0 medido no mesmo intervalo, nunca por tok/s absoluto; (c) a aceitação do MTP (por posição, do `/metrics`) não depende de contenção. O tok/s absoluto final do MiMo-9B sai do passe de tempo (seção 5.0c), com ele sozinho.
3. **Velocidade.** Prompts de agente do tipo que o MiMo-9B vai receber (entrada de 8 a 30 mil tokens com prefixo compartilhado, saída de 300 a 2000, 24 prompts fixos), a 1, 4 e 8 fluxos simultâneos (a carga do `qwen27b-mimo9b-fh` e do `mimo9b-direto` na execução principal). Por K: tok/s por fluxo (mediana), tok/s agregado, aceitação por posição e média de tokens aceitos por rodada.
4. **Assertividade.** A especulação é, em teoria, sem perda de qualidade (a distribuição de saída é preservada), então meça, não suponha: (a) **equivalência a temperatura 0**: nos mesmos 24 prompts, a fração de tokens iguais ao K=0 até a primeira divergência (esperado perto de 100%, com pequenas diferenças numéricas) e quantos prompts divergem antes de 256 tokens; (b) **aprovação em tarefas reais**: `fh direct` com o MiMo-9B em 20 exercícios do jg-eng-tests tirados do **fim** do catálogo, 2 repetições por K, corrigidos pelo grader oficial (resultado descartado do relatório principal, que reexecuta tudo depois), com a amostragem de sempre.
5. **Regra de escolha** (declarada antes de ver os dados). Descarte os K cuja aprovação fique mais de 2 exercícios (de 20) abaixo do K=0 ou cuja equivalência a temperatura 0 caia abaixo de 90% dos prompts sem divergência antes de 256 tokens. Dos que sobram, escolha o de **maior tok/s por fluxo, mediana a 4 fluxos, em razão ao K=0**; empate (menos de 5% de diferença) vai para o menor K. Registre a tabela inteira (todos os K, todas as métricas) e a decisão. Esse K é fixo para a sessão inteira.
6. Se nenhuma versão com MTP carregar no vLLM instalado (ou todas piorarem o K=0), o MiMo-9B roda **sem MTP** (K = 0), registrado como achado, e **a sessão continua**: nada é pulado, e o dono é avisado.

## 3. Fase 2: proxy de medição (obrigatório)
Escreva um proxy fino (Python + aiohttp ou equivalente) entre as configurações e o vLLM, na porta 8001, repassando para a 8000. Todas as configurações apontam para o proxy, nunca direto para o vLLM. Requisitos:

1. **Endpoints.**
   - `/v1/chat/completions`, `/v1/completions` e `/v1/models`, com e sem streaming (SSE repassado pedaço a pedaço, sem bufferizar).
   - `/v1/messages`: só se o vLLM instalado expuser; caso contrário, o tradutor da seção 5.5 fica entre o Claude Code e o proxy.
   - `GET /metrics`, repassado sem alteração (o `fh` lê a métrica para regular a concorrência).
   - Qualquer outro caminho: 404 registrado.
2. **Identificação da tarefa.** Uma chave de API aleatória por par (configuração, tarefa), gerada pelo operador e passada à configuração. O proxy mapeia chave → (configuração, tarefa) numa tabela em memória carregada de um arquivo que só o operador lê. No log vai o SHA-256 truncado da chave, nunca a chave. Se alguma configuração não permitir chave por tarefa, use o cabeçalho `x-client-id` ou a janela de tempo, e registre o método.
3. **Amostragem imposta.**
   - Em toda requisição, sobrescreva `temperature=1.0`, `top_p=0.95` e `top_k=20`, e registre os valores que o harness tinha enviado.
   - Não toque em `max_tokens`, `tools`, `tool_choice`, `chat_template_kwargs` nem `response_format`. Desligar o raciocínio em chamadas auxiliares é decisão de projeto do harness e deve aparecer no relatório; não é um desvio.
4. **Uso de tokens.**
   - Em streaming, se faltar `stream_options.include_usage`, injete-o, e remova do fluxo devolvido o pedaço extra de uso quando o cliente não o pediu.
   - Registre `prompt_tokens`, `completion_tokens` e `prompt_tokens_details.cached_tokens`.
   - O vLLM não separa tokens de raciocínio no `usage`: conte-os tokenizando o `reasoning_content` com o tokenizer do modelo e marque o campo como "estimado".
5. **Por requisição, grave em JSONL:**
   - horário de início e de fim (monotônico e de parede), configuração, tarefa, benchmark;
   - tempo até o primeiro token (primeiro delta de conteúdo *ou* de raciocínio, os dois registrados), duração;
   - tokens de entrada, saída, cache e raciocínio;
   - amostragem enviada e amostragem imposta;
   - número de ferramentas oferecidas e de tool calls devolvidas, `finish_reason`, status HTTP, erro.
6. **Fila.** Um coletor separado lê `/metrics` do vLLM a cada 5 s, na porta 8000 e não via proxy, e grava em JSONL:
   - `num_requests_running`, `num_requests_waiting`;
   - os histogramas de tempo em fila, TTFT e tempo por token de saída;
   - os contadores de decodificação especulativa: tokens de draft, aceitos e aceitos por posição;
   - a taxa de acerto do cache de prefixo.

   Confira os nomes exatos no `/metrics` da versão instalada. Em paralelo, grave `nvidia-smi --query-gpu=timestamp,utilization.gpu,memory.used,power.draw,temperature.gpu --format=csv -l 5`.
7. **Prova de isolamento de rede.** Os contêineres dos agentes ficam numa rede sem saída. Por exemplo, uma rede `--internal` do Docker em que só o contêiner do proxy (ou um encaminhador para a 8001 do host) é alcançável. Ou usuário dedicado + regra de firewall por dono (`iptables -m owner --uid-owner <uid>`) que aceita só 127.0.0.1:8001 e registra (`LOG`) e rejeita o resto. Guarde a contagem de pacotes bloqueados por configuração. Tentativas bloqueadas (telemetria, atualização automática) não são erro, mas entram no relatório.

8. **Cota por configuração (execuções em paralelo, seção 5.0).** O proxy limita as requisições em andamento de cada configuração a uma parte do `max-num-seqs` do servidor, para que uma configuração que abre muitos agentes (como `fh-memmax`) não deixe as outras sem GPU. A parte é `max(4, floor(max-num-seqs ÷ nº de configurações ativas))`; o que passar disso espera na fila do proxy, e o tempo de espera é registrado por requisição. Uma configuração que acaba os exercícios devolve a sua parte às outras.

9. **Segundo servidor (decisão 15).** O proxy também fica na frente do vLLM do MiMo-9B (porta 8002 do proxy para a 8003 do vLLM, com o seu próprio `/metrics` e o seu coletor). A **autorização é por chave**: só as chaves de `mimo9b-direto`, `qwen27b-mimo9b-puro` e `qwen27b-mimo9b-fh` alcançam o MiMo-9B; qualquer outra chave que peça o modelo `mimo9b` recebe 403 registrado, e a chave do `mimo9b-direto` nunca alcança o Qwen3.8-27B. A amostragem imposta é a mesma. O registro JSONL leva o campo `modelo` (27b ou 9b) e, para as duas do Qwen3.8-27B + MiMo-9B, o campo `papel` (planejador, operário, verificador), deduzido da ordem das chamadas da tarefa. A cota (item 8) vale por servidor: cada um tem o seu `max-num-seqs`.

Teste o proxy antes da fase 4:
- A chamada com ferramenta, a chamada em streaming e a leitura de `/metrics` passando pelo proxy.
- Uma requisição com `temperature=0.2` precisa chegar ao vLLM com 1,0.
- Uma tentativa de `curl https://example.com` de dentro do contêiner precisa falhar.
- Duas chaves de configurações diferentes, usadas ao mesmo tempo, aparecem separadas no log, e a cota de uma não consome a da outra.
- Uma chave do `fh-nomem` pedindo `mimo9b` recebe 403; uma chave do `mimo9b-direto` pedindo o Qwen3.8-27B também.

## 4. Fase 3: isolamento dos exercícios (por configuração)
1. Para cada uma das onze configurações, crie `WORKDIR/runs/<configuracao>/<exercicio>/` copiando apenas `exercise/` (nada é compartilhado entre configurações: cada uma tem a sua cópia). Em cada cópia, e igual para todas, rode:
   - `git init`, seguido de um commit único "base" (autor fixo). O `fh` precisa de um repositório git para checkpoints e diffs, e os outros harnesses também usam `git status`/`git diff`. Esse `.git` novo contém só o conteúdo do exercício; o `.git` do jg-eng-tests nunca é copiado.
   - `scripts/setup.sh`, **com rede** e **antes** de o agente começar (`npm ci` / `dotnet restore`), para que o agente rode sem rede. Use um cache de NuGet/npm montado em modo leitura depois do setup (um só cache de leitura pode ser compartilhado, porque ninguém escreve nele durante a execução). Se o setup falhar, é erro de infraestrutura daquele exercício, não da configuração.
2. Verificação por script, que bloqueia a fase se falhar. Nenhuma cópia pode conter `solution/`, `grader/`, `EVALUATION.md`, nenhum arquivo cujo hash coincida com um arquivo de `solution/` ou `grader/` que não exista também em `exercise/`, e nenhum `.git` com mais de um commit. Guarde a saída.
3. Cada exercício roda em contêiner próprio e descartável, com a cópia montada em `/work`, que também é o diretório de trabalho. Nada mais do disco do host fica montado, fora o cache de pacotes (só leitura), a configuração da configuração (só leitura) e o diretório de estado do `fh` daquele exercício (decisão 2; seção 5.0 e 5.1). Assim, "só pode ler e escrever dentro da sua pasta" vale para qualquer configuração, inclusive para comandos de shell. Cada exercício é uma sessão nova, com a conversa isolada; só a memória que o `fh` fundiu de exercícios anteriores da mesma configuração passa de um exercício para o outro.
4. Os graders ficam em `WORKDIR/src/jg-eng-tests`, fora de qualquer montagem dos agentes.

Prompt de tarefa, idêntico para todas as configurações (inclusive o `qwen27b-direto`, que não pode rodar nada e só devolve edições):
"Leia README.md e CHALLENGE.md desta pasta e implemente o que o desafio pede. Preserve as APIs públicas. Rode ./scripts/test.sh e ./scripts/lint.sh e corrija até passarem. Não acesse nada fora desta pasta. Ao terminar, resuma o que mudou e as suposições que fez."

## 5. Fase 4: execução (todas as configurações em paralelo, isoladas)

As onze configurações da decisão 10 rodam **ao mesmo tempo**, cada uma isolada das outras. A ordem importa só na fila e nos relatórios: a ordem é a da decisão 7 (sem harness, depois `fh`, depois os outros).

| # | configuração | o que é | seção |
|---|---|---|---|
| 1 | `mimo9b-direto` | **sem harness**: o MiMo-9B sozinho (`fh direct`) | 5.8 |
| 2 | `qwen27b-direto` | **sem harness**: o Qwen3.8-27B sozinho, a linha de base | 5.2 |
| 3 | `qwen27b-mimo9b-puro` (**Qwen3.8-27B + MiMo-9B, sem harness**) | o Qwen3.8-27B planeja, o MiMo-9B escreve uma vez, o Qwen3.8-27B verifica numa requisição sem ferramentas | 5.9 |
| 4 | `fh-nomem` | harness `fh`, 1 agente, cria e melhora skills mas **não usa** memória | 5.1 |
| 5 | `fh-mem1` | `fh`, 1 agente, cria, melhora e usa memória | 5.1 |
| 6 | `fh-memmax` | `fh`, paralelização máxima dentro da tarefa e um agente que consolida tudo, com memória | 5.1 |
| 7 | `qwen27b-mimo9b-fh` (**Qwen3.8-27B + MiMo-9B com `fh`**) | o Qwen3.8-27B planeja, o MiMo-9B escreve uma vez, o Qwen3.8-27B verifica e corrige com o `fh` completo; nunca devolve ao MiMo-9B | 5.7 |
| 8 | `opencode` | OpenCode | 5.3 |
| 9 | `qwen-code` | Qwen Code | 5.4 |
| 10 | `claude-code` | Claude Code | 5.5 |
| 11 | `deepseek` | DeepSeek Harness (UNSUPPORTED se não plugar) | 5.6 |

### 5.0 Mecânica comum e isolamento
- **Fila e orquestrador.** Um orquestrador (script do operador) mantém a fila de tarefas (configuração × exercício, na ordem do `catalog.json`) e a executa com o orçamento global de tarefas simultâneas G da seção 5.0a. Para cada tarefa: prepara a cópia, sobe o contêiner, roda a configuração, corrige com o grader e grava o resultado.
- **Estado.** `WORKDIR/state.json` com a situação de cada par (configuração, exercício): pendente, rodando, terminado, erro de infra, timeout. Se a sessão cair, retome pelo estado sem refazer o que terminou; um exercício que estava "rodando" é refeito do zero, com cópia nova.
- **Isolamento entre configurações (regra 10).** Para cada configuração:
  - contêiner próprio e descartável por tarefa, e uma **rede interna própria por configuração**, sem comunicação entre contêineres (`enable_icc=false`) e sem saída, só com a rota para o proxy;
  - **chave de API própria**, mapeada no proxy para a configuração e a tarefa;
  - **cotas** de CPU e memória por contêiner (`--cpus`, `--memory`): as do `task.toml` nos benchmarks e, no jg-eng-tests, 2 CPUs e 4 GB por tarefa (ajuste a todas igual se não couber);
  - **usuário do sistema próprio** (uid distinto) para o contêiner e para os arquivos que ela escreve;
  - diretórios próprios em `WORKDIR/runs/<configuracao>/`, `WORKDIR/estado/<configuracao>/` e `WORKDIR/grading/<configuracao>/`; nenhuma configuração lê o diretório de outra;
  - só se compartilham o vLLM, o proxy (que mede e limita, seção 3 item 8) e caches de pacotes em modo somente leitura.
- **Timeout de 900 s**: mate o grupo de processos inteiro (ou o contêiner) e marque `timeout`. O que ficou no disco é corrigido normalmente; timeout conta como tentativa válida, não como erro de infraestrutura.
- Guarde stdout/stderr de cada execução, o código de saída, o `git diff` final contra o commit "base" e a última mensagem do agente.
- **Correção**: copie a pasta final para `WORKDIR/grading/<configuracao>/<exercicio>/` e rode `python3 <jg-eng-tests>/exercises/<ex>/grader/run.py <cópia>`. Aprovado = `minimumPassed == true` no JSON, que já exige todos os critérios de nível mínimo e pelo menos 60% dos pontos. Guarde o JSON inteiro: critérios, pontos e diagnósticos. O grader tem timeout interno de 90 ou 120 s; se estourar, rode de novo uma vez e registre.
- **Erro de infraestrutura** (fora do denominador, listado com evidência): a configuração quebra antes da primeira chamada ao modelo (no `fh`, o JSON traz `verdict: error` com 0 requisições); o proxy ou o vLLM devolve 5xx ou fica inacessível; falha do runtime de contêineres, falta de memória ou disco; falha do `setup.sh`. Não são erro de infraestrutura: timeout, o agente desistir, tool call malformada, o agente estourar o contexto ou terminar sem mudar nada, e, no `qwen27b-direto`, edições que não casam com os arquivos.
- **Piloto.** Antes de rodar tudo, um piloto com 3 exercícios (um .NET, um React e um Angular, por exemplo 01, 07 e o primeiro Angular do catálogo) **para cada uma das onze configurações, rodando juntas**: valida cada configuração (a chamada chegou ao proxy com a chave certa, escreveu arquivos, rodou `test.sh`) e o isolamento entre elas. O piloto não conta. Se revelar erro de configuração, corrija a *configuração* (nunca o prompt de tarefa) e repita.
- **Por fase.** Em cada fase (jg-eng-tests e DeepSWE): vLLM novo, teste de fumaça, todas as configurações em paralelo, correção com o grader, parar o servidor.
- Se uma configuração falhar em mais de 5 dos 10 primeiros exercícios por erro de infraestrutura, primeiro diagnostique e conserte (quase sempre é erro nosso de configuração, não do harness) e repita o piloto. Só se o harness realmente não puder rodar nesta máquina, pare **só** essa configuração, marque UNSUPPORTED com a evidência, **avise o dono** e deixe as outras seguirem; o dono decide o que fazer com ela.

### 5.0b Julgamento final (feito pela LLM operadora)
A nota de cada exercício é decidida por você, a LLM que conduz os testes, em duas camadas. O modelo avaliado nunca participa.

**Camada 1: grader oficial (objetiva).** `grader/run.py` na cópia final, como descrito na seção 5.0. Aprovado no grader é condição necessária.

**Camada 2: revisão da LLM operadora (assertividade).** Para cada exercício aprovado pelo grader, e para uma amostra de 20% dos reprovados, leia o `git diff` final contra o commit "base" e o `CHALLENGE.md`. Decida se a entrega é legítima. Reprove, com o motivo registrado, se encontrar:
- testes públicos, scripts (`scripts/*.sh`), `global.json`, `package.json` de teste ou configuração de lint alterados, apagados ou desativados para passar;
- respostas fixas feitas para os casos de teste em vez da regra pedida;
- API pública quebrada ou renomeada, contra o que o enunciado pede;
- mudanças fora do escopo que introduzem risco: arquivos não relacionados reescritos, dependências novas sem motivo;
- trechos que o próprio enunciado proíbe: segurança desligada, validação removida etc.

A revisão é **cega**: um script do operador renomeia as entregas para códigos aleatórios (por exemplo `E017-H3`), inclusive as do `qwen27b-direto`, e remove do diff tudo que identifica a configuração, como `.fh/`, arquivos de sessão e comentários de assinatura. Só depois do julgamento de todos os harnesses o código é desfeito. Guarde a tabela de correspondência.

**Resultado por exercício:** `aprovado` = grader aprovou **e** a revisão confirmou. Reporte três números por configuração: aprovação no grader, aprovação final, e quantas aprovações do grader a revisão derrubou, com os motivos agrupados. O ranking principal usa a aprovação final. O ranking só pelo grader aparece ao lado, para comparar com as rodadas anteriores.

**Nota de qualidade (0–5)** para toda entrega aprovada: clareza, aderência ao enunciado, testes adicionados, simplicidade. Rubrica fixa, escrita antes de ver a primeira entrega e guardada em `relatorio/rubrica.md`. Não entra no critério de aprovação, só no relatório.

**Consistência do juiz.** Reavalie às cegas 10% das entregas, sorteadas com semente fixa, numa segunda passada separada da primeira, e reporte a concordância. Se ela ficar abaixo de 90%, revise a rubrica, reavalie tudo e registre.

**Só o resultado final conta (decisão 1).** O que o harness diz sobre si, as tentativas intermediárias e quantas rodadas ele precisou não entram no julgamento nem no relatório como acerto ou erro. Quem errou duas vezes e acertou na terceira está certo. Tempo e tokens gastos continuam sendo medidos como custo (seção 6).

### 5.0a Orçamento global de tarefas simultâneas (G): rampa de 5 em 5
O objetivo é terminar o conjunto mais rápido, com várias tarefas ao mesmo tempo, sem estragar nem o desempenho nem a comparação. G é o **total** de tarefas rodando ao mesmo tempo, somando as onze configurações. O uso extremo (muito além do ponto de parada da rampa) fica para a fase 8.

**Duas medidas decidem a rampa**, calculadas no proxy sobre a janela de cada degrau:
- **tok/s total** = tokens de saída de todas as requisições ÷ tempo da janela (a vazão do servidor);
- **tok/s por tarefa** = tok/s de geração de cada requisição, tokens de saída ÷ (duração − TTFT), **mediana** entre as requisições da janela (o que um agente vê enquanto o modelo gera). Registre também o P10 e o tok/s efetivo por tarefa (total ÷ G), que inclui o tempo em que a tarefa está rodando ferramentas e não gerando.

**A regra.** Os degraus são G = 5, 10, 15, 20, 25, 30, ... Passe ao degrau seguinte se, no degrau atual:
1. o tok/s total **cresceu pelo menos 5%** em relação ao degrau anterior (crescer menos que isso é saturação: mais carga não dá mais vazão, só mais latência); e
2. o tok/s por tarefa (mediana) **continua em 30 ou mais**.

**Pare** no primeiro degrau em que uma das duas falha, e N_MAX = o degrau anterior. Se o ganho do tok/s total ficar entre −3% e +5%, repita esse degrau mais uma vez e decida pela média das duas janelas (pelo menos +5% sobre o degrau anterior para passar), para o ruído não parar a rampa à toa nem deixá-la subir à toa. No primeiro degrau (G = 5) só vale o mínimo de 30 tok/s por tarefa. Registre qual das condições parou a rampa; se foi o teto duro, diga que o limite foi do ambiente e não do modelo.

**O servidor do MiMo-9B (decisão 15).** G conta as tarefas ativas no servidor do Qwen3.8-27B; as chamadas do MiMo-9B contam no servidor dele, que tem o seu próprio `max-num-seqs` e a sua própria rampa, pela mesma regra (cresceu 5% ou mais e mediana por fluxo ≥ 30), medida na mesma calibração com `mimo9b-direto` e `qwen27b-mimo9b-fh` entrando como as outras configurações. O N_MAX do MiMo-9B limita só as tarefas dessas duas configurações no operário; na prática o MiMo-9B atende rajadas curtas (uma chamada por tarefa), e o gargalo costuma ser o Qwen3.8-27B. Registre os dois.

**Freios de segurança** (valem junto, por proteção; não são a regra de desempenho): erro 5xx, falta de memória, preempção, ou KV cache acima de 95% de forma sustentada. Qualquer um deles rejeita o degrau, e N_MAX = o degrau anterior. Há também um **teto duro**, para a rampa não correr sem limite: o menor entre o `max-num-seqs` do servidor, os núcleos livres do host ÷ as CPUs de cada tarefa, e 60 (esse último é só um limite de tempo e de disco; se o tok/s ainda estiver crescendo nele, isso é uma boa notícia, e o N_MAX fica no teto).

**1. Calibração (uma vez, antes de tudo, fora da comparação).**
- Servidor recém-iniciado, com `fh-mem1` sozinho nos primeiros exercícios do catálogo (os que forem necessários; as execuções não contam, e a memória do `fh` usada aqui é descartada).
- Degraus: G = 1 (referência: velocidade de um fluxo sozinho, que serve de comparação), depois 5, 10, 15, ... pela regra acima. Cada degrau dura pelo menos 6 minutos de carga constante **e** pelo menos G tarefas concluídas, o que vier por último (no máximo 15 minutos); o degrau de referência G = 1 dura 3 minutos. Uma tarefa nova só entra quando outra termina, para manter G constante.
- No fim de cada degrau, imprima e grave em `WORKDIR/calibracao.csv`:
  - tok/s total, e tok/s por tarefa (mediana, P10 e efetivo);
  - requisições rodando e em fila, média e pico;
  - uso de KV cache, média e pico, e preempções;
  - TTFT P50 e P90;
  - aceitação do MTP;
  - uso de CPU e RAM do host (builds de .NET e Node em paralelo competem por CPU);
  - potência média da GPU;
  - erros e timeouts.
- Registre a decisão com os números: tabela e gráfico de tok/s total e por tarefa contra G, marcando o degrau que parou a rampa e o motivo.
- **Checagem de mistura.** Depois da calibração, rode as onze configurações juntas **em G = N_MAX** por 10 minutos (descartada). Ela valida o proxy, as cotas e o isolamento sob a mistura real, que é mais pesada que o `fh-mem1` sozinho (o `fh-memmax` abre scouts e workers). Aplique a mesma regra (tok/s por tarefa ≥ 30 e freios de segurança); se falhar, desça 5 e repita, até passar. O G que passar é o de partida da execução principal.

**2. Escalonamento na execução principal.**
- **Fila justa, na ordem da decisão 7.** A fila gira pelas configurações na ordem da tabela (`mimo9b-direto`, `qwen27b-direto`, `qwen27b-mimo9b-puro`, depois `fh-nomem`, `fh-mem1`, `fh-memmax`, `qwen27b-mimo9b-fh`, e por último `opencode`, `qwen-code`, `claude-code`, `deepseek`). Quando uma vaga abre, ela vai para a próxima configuração da rotação que tenha tarefa pendente e esteja abaixo do seu limite. O limite de cada configuração é `max(1, ceil(G ÷ nº de configurações ativas))`; com G = 5 e onze configurações, as cinco primeiras da rotação rodam e as outras esperam a vez, e o rodízio faz todas avançarem juntas.
- **G começa no valor que passou na checagem de mistura** (N_MAX ou menos; decisão 4), sem repetir a rampa. Uma janela de guarda de 15 minutos confere continuamente o mínimo de 30 tok/s por tarefa e os freios de segurança; se a guarda falhar, G **desce 5 e fica congelado** nesse valor pelo resto da fase, sem oscilar; registre o horário e o motivo. G nunca sobe na execução principal.
- A cada 10 tarefas concluídas (e sempre que G mudar), imprima a mesma linha de números da calibração, com o G atual, e grave-a em `WORKDIR/runs/<configuracao>/tps.csv` e em `WORKDIR/tps-global.csv`.
- **Freio de segurança na execução.** Se aparecer erro 5xx, falta de memória ou KV cache acima de 95% por mais de 2 minutos, pare de lançar tarefas novas até a fila esvaziar e continue no mesmo G. Nunca mude o servidor, e registre cada acionamento com o horário. Se o freio acionar três vezes, avise no relatório que o G ficou alto demais para a mistura.
- O timeout continua 900 s de parede. Reporte a taxa de timeout por faixa de G.
- As requisições extras do `fh-memmax` (scouts e workers) **não** contam em G: elas passam pela cota do proxy (seção 3 item 8) e a espera aparece como fila no relatório. Elas pesam nas medidas da regra (tok/s por tarefa inclui os fluxos dos scouts e workers). Não reduza o `fh-memmax` por isso; a disputa pela GPU é o custo da paralelização.

### 5.0c Passe de tempo: velocidade comparável (cada configuração sozinha)
Como as configurações dividem a GPU na execução principal, o tempo, o TTFT e o tok/s dela ficam contaminados (regra 4). Para ter velocidade comparável:
- **Quando.** Depois da execução principal do jg-eng-tests, com o vLLM reiniciado e **uma configuração por vez**, na mesma ordem da tabela.
- **O quê.** Um conjunto fixo de 20 exercícios (8 .NET, 6 React, 6 Angular), escolhido com semente registrada **antes** da primeira execução, o mesmo para todas, com 4 tarefas simultâneas dentro da configuração (o mesmo valor para todas; registre). São ~40 minutos por configuração.
- **Memória.** O `fh-mem1` e o `fh-memmax` começam esse passe com a memória **vazia**: o armazém da execução principal já viu esses exercícios, e usá-lo vazaria resposta para a medição. O resultado de qualidade desse passe é reportado à parte e não entra no ranking.
- **Métricas de velocidade.** O tempo por tarefa (média, mediana, P90), o TTFT, o tok/s por fluxo e a aceitação do MTP vêm **só deste passe**. Os tokens por tarefa e a qualidade vêm da execução principal.
- **As duas do MiMo-9B.** `mimo9b-direto` e `qwen27b-mimo9b-fh` entram no passe, cada uma sozinha, com os dois servidores de pé (o que não está em uso fica ocioso). A velocidade do MiMo-9B sozinho (tok/s por fluxo, aceitação do MTP com o K escolhido) sai daqui; no `qwen27b-mimo9b-fh` registre o tempo de cada etapa (planejador, operário, verificador).
- **DeepSWE.** O passe de tempo também é feito no DeepSWE, com 10 tarefas de semente fixa, cada configuração sozinha.

### 5.1 Configurações do `fh` (`fh-nomem`, `fh-mem1`, `fh-memmax`)
As três usam o mesmo binário, o mesmo modelo, o mesmo proxy, a mesma amostragem e o mesmo comando. Só mudam três chaves de configuração, num diretório só de leitura por configuração, `WORKDIR/fh-config/<configuracao>/config.json`, passado com `FH_CONFIG_HOME`:

```json
{
  "endpoint": "http://<proxy>:8001/v1",
  "model": "frankenstein-v2",
  "contextWindow": 131072,
  "maxConcurrency": 1,
  "memory": "use",
  "consolidate": false,
  "telemetry": false,
  "sampling": {
    "thinking": {"temperature": 1.0, "topP": 0.95, "topK": 20},
    "instant":  {"temperature": 1.0, "topP": 0.95, "topK": 20}
  }
}
```

| configuração | `maxConcurrency` | `memory` | `consolidate` |
|---|---|---|---|
| `fh-nomem` | 1 | `"off"` | `false` |
| `fh-mem1` | 1 | `"use"` | `false` |
| `fh-memmax` | 16 | `"use"` | `true` |

- `contextWindow` deve ser igual ao `max-model-len` usado. Deixe `maxTaskTokens` no padrão (800 000) nas três e registre; o `fh` para ao atingir esse limite, e ele inclui os tokens dos scouts e workers.
- **`fh-nomem` (sem memória).** O `fh` continua **criando, melhorando e enriquecendo** skills a cada tarefa verificada, e o armazém dele cresce como o das outras, mas **nenhuma skill aprendida entra no prompt**. Os pacotes embutidos do `fh` (guia por versão de framework) continuam ligados nas três, porque fazem parte do harness e não da memória. O JSON de cada tarefa traz `skillsWithheld`: as skills aprendidas que o `fh` teria usado. Como a melhoria de uma skill nasce de tarefas em que ela foi selecionada, aqui ela evolui "como se tivesse sido usada"; registre essa ressalva.
- **`fh-mem1` (com memória, um agente).** Igual ao `fh-nomem`, mas as skills aprendidas entram no prompt das tarefas seguintes. A diferença entre as duas é, por construção, o efeito da memória.
- **`fh-memmax` (paralelização máxima, com consolidação).** Igual ao `fh-mem1`, mais paralelização dentro de cada tarefa e **um único agente que consolida tudo**:
  - **Trabalho que se divide** em partes de arquivos disjuntos: workers rodam em paralelo (até 16, regulados pelo `/metrics`), e depois um agente consolidador revisa o resultado combinado, reconcilia o que ficou inconsistente entre as partes, faz as edições que os workers pediram em arquivos de outros, roda as verificações do projeto e fecha.
  - **Trabalho que não se divide** (o caso comum de uma issue): até 4 *scouts* somente-leitura rodam em paralelo (modo plano: não conseguem editar), cada um num ângulo (Localizar, Testes, Impacto, Solução) e devolvendo achados e uma proposta de solução. Um único agente consolida os relatórios, escolhe a solução mais bem fundamentada, implementa **uma** mudança coerente e verifica.
  - Registre por tarefa: scouts que responderam, workers criados, tokens por agente, o campo `consolidation` do JSON, e a espera na fila do proxy. Registre também em quantas tarefas não houve nada a paralelizar.
- Os dois campos novos têm equivalentes por linha de comando e variável de ambiente (`--no-memory` ou `FH_MEMORY=off`, `--consolidate` ou `FH_CONSOLIDATE=1`, `FH_MAX_CONCURRENCY`); use o arquivo de configuração, para que a configuração fique registrada.

Comando por exercício, dentro do contêiner, com `/work` como diretório:

```bash
FH_CONFIG_HOME=/fh-config \
FH_HOME=/fh-state \
FH_API_KEY=<chave da tarefa> \
FH_METRICS_URL=http://<proxy>:8001/metrics \
fh run --auto --yes --mode yolo --json --cwd /work "<prompt de tarefa>"
```

Pontos obrigatórios:
- **Memória com um só dono, por configuração (decisão 2).**
  - **Nunca deixe vários processos ou contêineres escreverem no mesmo banco.** O banco de skills é um arquivo SQLite. Quem o cria primeiro define as permissões (por padrão 0640, só do dono), e os outros usuários ou uids recebem `attempt to write a readonly database`; além disso, os arquivos `-wal` e `-shm` compartilhados entre contêineres são frágeis. O `fh` agora não aborta nesse caso (usa uma cópia privada e avisa), mas aí o aprendizado daquele processo se perde.
  - **Layout.** `WORKDIR/fh-state/<configuracao>/shared/` é o armazém compartilhado **daquela configuração**, gravado só pelo orquestrador (um único usuário). Cada exercício recebe uma cópia privada: antes de iniciar o contêiner, `python3 -m fh_harbor.sync snapshot --from WORKDIR/fh-state/<configuracao>/shared --to WORKDIR/fh-state/<configuracao>/tasks/<exercicio>`, e esse diretório é montado como `/fh-state` (leitura e escrita), fora de `/work`, para não aparecer no diff nem na correção.
  - **Ao terminar o exercício**, com a trava de arquivo: `python3 -m fh_harbor.sync merge --into WORKDIR/fh-state/<configuracao>/shared WORKDIR/fh-state/<configuracao>/tasks/<exercicio>`. A fusão guarda a skill mais nova, acrescenta o histórico uma vez e leva milissegundos; erros de fusão não derrubam o exercício, entram no log. Isso equivale a "guardar a memória só ao final de cada tarefa", sem o risco de perder tudo se o processo cair no meio.
  - **Quem aprende o quê.** Uma tarefa vê o que as tarefas já **fundidas** da mesma configuração aprenderam antes de ela começar; o que tarefas paralelas aprendem ao mesmo tempo só chega às seguintes. A ordem exata do aprendizado não é determinística; registre isso.
  - Cada uma das três configurações começa com o armazém **vazio** e nunca lê o das outras. O armazém do `fh-nomem` é guardado ao final e não é usado em nenhuma configuração.
  - **Se mesmo assim vários usuários tiverem de escrever direto no mesmo diretório** (não recomendado): crie o diretório com `chmod 2775`, o grupo comum e `umask 002`, ou rode o `fh` com `FH_SHARED_STORE=1`. Mantenha em disco local, nunca em NFS. Qualquer membro do grupo passa a poder alterar as skills.
  - Ao terminar cada configuração, guarde uma cópia de `WORKDIR/fh-state/<configuracao>/shared/` e a saída de `fh activity` e `fh stats` (com `FH_HOME` apontando para ela), e os `tasks/<exercicio>/`, que têm as sessões de cada exercício.
- **Sem `--keep` (decisão 1).** O que o `fh` deixar no disco ao terminar é a resposta dele: se a verificação interna reprovou e ele corrigiu, ótimo; se reprovou até o fim e ele desfez a mudança, a entrega é o estado original. Não corrija patches rejeitados nem estados intermediários: só a entrega final importa. Antes da correção e da revisão às cegas, apague `.fh/` da cópia, porque lá ficam os patches rejeitados.
- **Sem `--sandbox`** na comparação principal: o contêiner já isola todas as configurações igualmente.
- **Verificação com os scripts do exercício (decisão 3, já corrigido no `fh`).** Quando o repositório tem `scripts/test.sh`, o `fh` verifica com os scripts que o próprio repositório declara (`scripts/build.sh`, `typecheck.sh`, `lint.sh`, `test.sh`, na ordem, com `bash`), em vez de inferir `dotnet test` ou `npm test`. No jg-eng-tests isso dá `bash scripts/lint.sh` e `bash scripts/test.sh`. Confirme nos pilotos (fase 0, item 6). Se ainda aparecer comando inferido, pare e corrija antes da execução que conta.
- Guarde o JSON de `--json` como dado bruto (não entra na nota): rodadas de verificação, checks executados, workers, scouts, tokens, tool calls, chamadas reparadas ou malformadas, skills usadas (`skillsUsed`) e retidas (`skillsWithheld`), aceitação do MTP.

### 5.2 `qwen27b-direto`: o modelo sozinho, sem harness (linha de base)
Responde a uma pergunta simples: cada harness (o `fh` inclusive) melhora ou piora o modelo? É o **mesmo modelo, o mesmo proxy, a mesma amostragem e o mesmo prompt de tarefa**, chamado diretamente.
- **O que é.** `fh direct`, que só usa o cliente HTTP do `fh`: **uma requisição por tarefa**, sem ferramentas, sem laço de agente, sem verificação, sem skills e sem segunda tentativa. A requisição leva um instantâneo do repositório (a árvore de arquivos; a documentação da raiz, como README e CHALLENGE; os arquivos citados no enunciado; e, com o espaço que sobrar, até cerca de 60% da janela, os demais arquivos, os menores primeiro). O modelo responde com blocos SEARCH/REPLACE, que são aplicados como vieram; um SEARCH vazio cria arquivo.
- **Comando**, dentro do contêiner, com `/work` como diretório:

```bash
FH_CONFIG_HOME=/fh-config FH_API_KEY=<chave da tarefa> fh direct --cwd /work "<prompt de tarefa>"
```
  (`--commit` nos benchmarks que corrigem o `HEAD`, como o DeepSWE.) O arquivo de configuração é o mesmo da seção 5.1; `maxConcurrency`, `memory` e `consolidate` não fazem nada aqui.
- **Mesmo juiz.** O resultado passa pelo mesmo grader e pela mesma revisão às cegas (5.0b), como todas as outras. O `verdict` do JSON (`applied` ou `failed`) é só informativo.
- **Limites, para o relatório.**
  - Ele não roda nada: não executa testes nem o lint. Edições que não casam com o arquivo ficam em `direct.failed` do JSON e contam como erro do modelo, não da infraestrutura.
  - Em repositórios grandes (DeepSWE) o instantâneo não cabe inteiro; registre `contextFiles` e `repoFiles` por tarefa.
  - Se a resposta nem chegar (endpoint fora do ar), o JSON traz `verdict: error` com 0 requisições e conta como erro de infraestrutura.
- **Como o relatório usa a linha de base.** Para cada configuração, a diferença para o `qwen27b-direto` em taxa de aprovação (com intervalo e McNemar), em tokens por tarefa aprovada e em tempo (do passe de tempo): melhorou, piorou ou sem diferença conclusiva.

### 5.3 OpenCode
Provedor OpenAI-compatível (`@ai-sdk/openai-compatible`) apontando para `http://<proxy>:8001/v1`, modelo `frankenstein-v2`, com a chave do exercício. Confira a sintaxe de provedor customizado e do modo não interativo (`opencode run`) na documentação da versão instalada. Use as mesmas permissões da rodada anterior: bash restrito a `scripts/test.sh`, `lint.sh`, `setup.sh`, `git status`/`git diff` e `pwd`; sem web, sem subagentes, sem skills. Desligue compartilhamento e atualização automática. Registre a configuração exata (sem a chave).

### 5.4 Qwen Code
Via `OPENAI_BASE_URL=http://<proxy>:8001/v1`, `OPENAI_API_KEY=<chave do exercício>` e `OPENAI_MODEL=frankenstein-v2`, em modo não interativo (`qwen -p "<prompt>"`, com aprovação automática de ferramentas: confira a flag na versão instalada). Sem web, sem MCP, telemetria desligada.

### 5.5 Claude Code
- **Endpoint.** Precisa de endpoint no formato da API da Anthropic. Verifique se o vLLM instalado expõe `/v1/messages`. Se não expuser, use um tradutor, por exemplo LiteLLM com rota `/v1/messages` → provedor `hosted_vllm` apontando para o proxy. A cadeia fica Claude Code → LiteLLM → proxy → vLLM, e o proxy continua sendo o ponto de medição. Registre a versão do LiteLLM.
- **Modelos.** Configure `ANTHROPIC_BASE_URL` para ele e aponte todos os slots de modelo para `frankenstein-v2`: `ANTHROPIC_MODEL`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL` e o slot "rápido" da versão instalada. Use a chave do exercício em `ANTHROPIC_AUTH_TOKEN` ou `ANTHROPIC_API_KEY`, conforme a versão.
- **Tráfego externo.** Defina `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` e `DISABLE_TELEMETRY=1`.
- **Execução.** Não interativo: `claude -p "<prompt>" --output-format json`, com permissões equivalentes às do OpenCode via `--allowedTools`/`--permission-mode`. Confira as flags com `claude --help`.
- **Prova.** Confirme no log do proxy e no firewall que nenhuma chamada saiu para a Anthropic.

### 5.6 DeepSeek Harness
Localize o repositório oficial e verifique se aceita endpoint OpenAI-compatível customizado e tool calling com este modelo. Antes de desistir, tente plugá-lo: um adaptador próprio (um shim OpenAI-compatível, ou o agente por import path do Harbor/Pier, como o `fh_harbor` do repositório) e registre a tentativa. Se de fato não aceitar, marque UNSUPPORTED com a evidência (trecho de código ou documentação, versão), **avise o dono** e siga. Se aceitar, use modo não interativo, com as mesmas permissões e sem web.

### 5.7 `qwen27b-mimo9b-fh` ("Qwen3.8-27B + MiMo-9B"): o Qwen3.8-27B planeja e verifica, o MiMo-9B escreve (decisão 15)
Pergunta: um operário barato e rápido que escreve o código, com o Qwen3.8-27B só planejando e verificando, ganha desempenho (qualidade, tempo, tokens do Qwen3.8-27B) contra o próprio Qwen3.8-27B? **Sempre rotulado "Qwen3.8-27B + MiMo-9B" nas tabelas.**
- **O que é.** `fh delegate`, três etapas de mão única, sem laço de volta:
  1. **Planejador (Qwen3.8-27B).** Recebe o instantâneo do repositório e a tarefa (como no `fh direct`) e escreve uma **ordem de trabalho**: causa ou desenho, arquivos e funções, comportamento exigido, armadilhas. Não escreve o patch.
  2. **Operário (MiMo-9B).** Recebe o instantâneo, a tarefa e a ordem de trabalho e responde com blocos SEARCH/REPLACE (o formato do `fh direct`), aplicados como escritos. **Uma chamada. O MiMo-9B nunca é chamado de novo**, qualquer que seja o resultado, inclusive se nenhuma edição aplicar.
  3. **Verificador (Qwen3.8-27B).** Recebe o repositório já com o trabalho do MiMo-9B e roda o `fh` completo (ferramentas, testes, lint, laço de verificação e correção, `fh run`) com um prompt que diz que um operário fez uma primeira tentativa e que ele deve revisar, rodar as verificações e consertar ou terminar sozinho. É o Qwen3.8-27B que entrega.
- **Configuração do `fh` na etapa 3:** igual à do `fh-nomem` (um agente, `FH_MEMORY=off` com skills criadas e nunca usadas, sem consolidação), para que `qwen27b-mimo9b-fh` contra `fh-nomem` isole só o efeito do MiMo-9B. Tem o seu próprio estado, vazio.
- **Comando**, dentro do contêiner, com `/work` como diretório:

```bash
FH_CONFIG_HOME=/fh-config FH_MEMORY=off FH_API_KEY=<chave da tarefa> \
FH_WORKER_ENDPOINT=http://<proxy>:8002/v1 FH_WORKER_MODEL=mimo9b FH_WORKER_CONTEXT_WINDOW=<igual ao do MiMo-9B> \
fh delegate --auto --yes --mode yolo --json [--commit] --cwd /work "<prompt de tarefa>"
```
  (`--commit` nos benchmarks que corrigem o `HEAD`.) Harbor/Pier: `mode=delegate` com `worker_endpoint`, `worker_model` e `worker_context_window`; no Pier o endereço do MiMo-9B também entra na lista de hosts permitidos (o adaptador já faz isso).
- **O que medir a mais.** O JSON traz `delegate`: tokens e requisições do planejador e do operário, as edições do MiMo-9B (`edits`, `changed`, `failed`) e a ordem de trabalho. Meça, por tarefa: a fração do trabalho do MiMo-9B que **sobreviveu** até o diff final (linhas do MiMo-9B que o Qwen3.8-27B manteve), quantas vezes o Qwen3.8-27B teve de corrigir, tempo gasto em cada etapa, tokens de saída do MiMo-9B contra os do Qwen3.8-27B (o ganho de custo é o código que o Qwen3.8-27B deixou de escrever) e a aprovação com a ordem de trabalho e o MiMo-9B contra sem ele (`fh-nomem`).
- **Mesmo juiz** (5.0b). O verificador entrega o resultado; o relatório não conta tentativas do MiMo-9B como erro. A revisão às cegas também confere que o diff não traga artefatos do MiMo-9B (comentários de operário, código morto).
- **Falha de infraestrutura:** se o MiMo-9B (ou o Qwen3.8-27B) está fora do ar, o JSON traz `verdict: error` e a tarefa é refeita; se o MiMo-9B responde mas sem edições aplicáveis, o Qwen3.8-27B segue e termina sozinho, e isso conta como resultado da configuração (e aparece na taxa "MiMo-9B sem edição aplicável").

### 5.8 `mimo9b-direto`: o MiMo-9B sozinho (decisão 15)
Igual à seção 5.2, trocando o modelo: `fh direct`, uma requisição por tarefa, sem ferramentas, laço, verificação nem skills, **no servidor do MiMo-9B, com o MTP escolhido na seção 2.2**, o mesmo prompt de tarefa, o mesmo proxy e a mesma amostragem, rodando em todos os conjuntos (jg-eng-tests e DeepSWE) e passando pelo mesmo grader e pela revisão às cegas. Comando: o da seção 5.2 com `FH_ENDPOINT=http://<proxy>:8002/v1 FH_MODEL=mimo9b FH_CONTEXT_WINDOW=<do MiMo-9B>`. Serve de linha de base do operário: `qwen27b-mimo9b-fh` contra `mimo9b-direto` mostra o que o Qwen3.8-27B acrescenta ao MiMo-9B, e contra `qwen27b-direto` e `fh-nomem` mostra se o MiMo-9B acrescentou algo ao Qwen3.8-27B. O passe de tempo (5.0c) mede o tok/s do MiMo-9B sozinho.

### 5.9 `qwen27b-mimo9b-puro` ("Qwen3.8-27B + MiMo-9B, sem harness"): a dupla sem ferramentas (decisão 15)
O mesmo fluxo de mão única da seção 5.7, mas **sem harness em nenhuma etapa**: sem ferramentas, laço de agente, testes, skills ou memória.
1. **Planejador (Qwen3.8-27B):** uma requisição com o instantâneo e a tarefa devolve a ordem de trabalho.
2. **Operário (MiMo-9B):** uma requisição com o instantâneo, a tarefa e a ordem de trabalho devolve blocos SEARCH/REPLACE, aplicados como escritos. **Nunca é chamado de novo.**
3. **Verificador (Qwen3.8-27B):** uma requisição, **sem ferramentas**, com o repositório já alterado pelo MiMo-9B, a tarefa e a ordem de trabalho. Ele só lê (não roda código): responde `NO CHANGES` ou blocos SEARCH/REPLACE com as correções, aplicados como escritos.
Total: **três requisições por tarefa**, duas no Qwen3.8-27B e uma no MiMo-9B. Comando (igual ao da seção 5.7, com `--no-harness`):

```bash
FH_CONFIG_HOME=/fh-config FH_API_KEY=<chave da tarefa> \
FH_WORKER_ENDPOINT=http://<proxy>:8002/v1 FH_WORKER_MODEL=mimo9b FH_WORKER_CONTEXT_WINDOW=<do MiMo-9B> \
fh delegate --no-harness [--commit] --cwd /work "<prompt de tarefa>"
```
Harbor/Pier: `mode=delegate-pure`. O JSON traz `delegate` (planejador e operário) e `verifier` (edições, arquivos alterados, falhas, tokens). Mesmo juiz (5.0b). Como no `qwen27b-direto`, edições que não casam com os arquivos são erro do modelo, e `verdict: error` com 0 requisições é erro de infraestrutura. O contraste que importa: a dupla sem harness contra cada um sozinho (decisão 15, contraste 4 da seção 7).

## 6. Fase 5: o que medir (por exercício e agregado por configuração)
- **Qualidade:**
  - aprovado, nota (score/maximum), critérios aprovados por nível (mínimo, esperado, excelente);
  - erros de infraestrutura (fora do denominador, listados), timeouts;
  - aprovação no grader, aprovação final (grader + revisão da LLM operadora) e aprovações derrubadas pela revisão, com os motivos;
  - nota de qualidade 0–5 da LLM operadora;
- **Tempo** (comparável só no passe de tempo, seção 5.0c, com cada configuração sozinha; na execução principal o tempo é contaminado pela carga das outras e entra no relatório só como informação):
  - tempo de parede por exercício: média, mediana, P90;
  - tempo até o primeiro token: mediana e P90 por requisição.
- **Tokens:**
  - entrada, cache, entrada sem cache, saída e raciocínio (estimado);
  - total por exercício e por configuração (inclui scouts e workers do `fh-memmax`);
  - tokens por exercício aprovado, total e sem cache.
- **Velocidade por degrau de G (de 5 em 5, até onde a rampa parou):**
  - tok/s por fluxo e agregado, fila, KV cache, TTFT, aceitação do MTP e potência, em cada degrau, para a mistura das onze configurações;
  - a curva de tok/s agregado contra G é o gráfico principal desta parte.
- **Velocidade por configuração** (só do passe de tempo da seção 5.0c):
  - tok/s por fluxo (saída ÷ (duração − TTFT)) e agregado;
  - concorrência média e de pico (do log do proxy);
  - fator de paralelização: soma das durações das requisições ÷ tempo de parede do exercício, descontando o tempo em fila;
  - aceitação do MTP, geral e por posição.
- **Comportamento:**
  - turnos, requisições, chamadas de ferramenta, tool calls malformadas;
  - execuções de `test.sh`/`lint.sh`, contadas nos logs da configuração ou nos comandos registrados (o `qwen27b-direto` não executa nada);
  - exercícios que terminaram sem declarar conclusão ou sem nenhuma mudança;
  - `fh`: agentes, skills aprendidas, usadas (`skillsUsed`) e retidas (`skillsWithheld`, só no `fh-nomem`) por exercício, e no `fh-memmax` scouts, workers e o campo `consolidation` (só para entender custo e aprendizado; não entram na nota);
  - `qwen27b-direto`: edições aplicadas e edições que não casaram (`direct`), e arquivos do instantâneo contra arquivos do repositório;
  - tentativas de rede bloqueadas.
- **Hardware:**
  - uso médio e de pico de GPU, VRAM, potência média, temperatura;
  - CPU e RAM do host por degrau, para separar gargalo de GPU de gargalo de build;
  - energia por configuração (integral da potência no tempo; só no passe de tempo, onde a GPU é de uma configuração por vez) e energia por exercício aprovado.

## 7. Fase 6: análise e relatório

### Análise
- **Ranking.** Taxa de aprovação final (grader + revisão da LLM operadora) com intervalo de Wilson de 95%. Ao lado, o ranking só pelo grader.
- **Comparação pareada.** Exercício a exercício, entre cada par de configurações: teste binomial exato bicaudal nos pares discordantes (McNemar exato) e intervalo de 95% da diferença de taxas por bootstrap pareado (10 000 reamostragens, semente fixa registrada). Corrija as comparações múltiplas por Holm. Não diga "melhor" sem apoio estatístico; use "sem diferença conclusiva" quando for o caso. Com 130 exercícios, diferenças de poucos pontos quase nunca são conclusivas; diga isso.
- **A cadeia de ablação (decisão 10).** Cinco contrastes planejados, declarados antes de ver os dados, cada um com diferença de taxa de aprovação, intervalo e McNemar, tokens por exercício aprovado e tempo (do passe de tempo):
  1. **O harness:** `fh-nomem` contra `qwen27b-direto`. O harness melhora ou piora o modelo?
  2. **A memória:** `fh-mem1` contra `fh-nomem`. Aprender ajuda quando se usa o que aprendeu? Mesmo harness, mesmo agente; só o uso da memória muda.
  3. **A paralelização com consolidação:** `fh-memmax` contra `fh-mem1`. Vale o custo em tokens e fila? Aplique a regra de fan-out do projeto (`docs/FANOUT.md`): compensa se não perder qualidade e se (a) a mediana de tempo ficar ≤ 80% da do `fh-mem1`, ou a aprovação subir pelo menos 5 pontos, e (b) os tokens sem cache por exercício aprovado ficarem ≤ 2× os do `fh-mem1`. Mostre também o resultado só nas tarefas em que houve scouts ou workers.
  4. **O MiMo-9B como operário.** Sem harness: `qwen27b-mimo9b-puro` contra `qwen27b-direto` (Qwen3.8-27B sozinho) e contra `mimo9b-direto` (MiMo-9B sozinho): juntos superam cada um sozinho? Com harness: `qwen27b-mimo9b-fh` contra `fh-nomem` (mesma etapa de verificação, com e sem o MiMo-9B escrevendo antes) e contra `qwen27b-direto` (o Qwen3.8-27B sozinho); e `qwen27b-mimo9b-fh` contra `qwen27b-mimo9b-puro` (o que o `fh` acrescenta à dupla). Responda: o operário trouxe ganho de qualidade, de tempo, de tokens do Qwen3.8-27B, ou só custo?
  5. **Os outros harnesses contra a linha de base:** `opencode`, `qwen-code`, `claude-code` e `deepseek` contra `qwen27b-direto`, e contra o melhor `fh`.

  Uma tabela única mostra, para cada configuração, a diferença para o `qwen27b-direto`: melhorou, piorou ou sem diferença conclusiva. Os cinco contrastes formam a família do ajuste de Holm (o contraste 4 tem cinco comparações, então a família soma nove).
- **Recortes.** Resultados por trilha, stack (.NET, React, Angular) e nível.
- **Por tipo.** Exercícios cujo código inicial devolve `<unimplemented>` (contrato de saída implícito) contra os que já têm código real.
- **Escala.** O que a paralelização entre tarefas (G) rendeu:
  - tempo total de parede real contra o de rodar uma tarefa por vez (soma das durações);
  - tok/s agregado e por fluxo por degrau de G;
  - onde a curva achata e o motivo: KV cache, fila ou queda da aceitação do MTP.
  - Diga também se a aprovação mudou depois de G descer (se isso aconteceu). Como G só desce na execução principal, qualquer diferença mistura dificuldade com concorrência. Aponte a confusão e não conclua causalidade.
- **Aprendizado do `fh` (decisão 2).** A comparação limpa é o contraste 2 (`fh-mem1` contra `fh-nomem`). Além dela:
  - a taxa de aprovação na primeira metade contra a segunda metade do catálogo, nas três configurações do `fh` (se a memória ajuda, a curva do `fh-mem1` sobe mais que a do `fh-nomem`);
  - nos exercícios em que alguma skill aprendida foi usada (`fh-mem1`) ou teria sido (`skillsWithheld` do `fh-nomem`), contra os demais;
  - quantas skills foram aprendidas, melhoradas, promovidas e postas em quarentena em cada configuração, e se os armazéns de `fh-nomem` e `fh-mem1` divergiram (a qualidade das skills cresce igual sem uso?).
  - Não conclua causalidade pela ordem: ela mistura dificuldade com aprendizado.
- **Com repetições.** Se houver repetições, reporte a média por exercício e a variância entre elas. As estatísticas pareadas usam a taxa média por exercício.
- **Referência externa**, marcada como ambiente diferente e fora das estatísticas: a rodada anterior do Frankenstein V2 com harness caseiro (57,7%, temperatura 0,2) e os resultados do Claude Opus 5.5 (50,0%) e do Sonnet 5.5 (47,7%) no Claude Code.

### Avaliação independente das instruções e dos testes
Feita pela LLM operadora, nunca pelo Qwen. Só o operador lê os graders, e só depois das execuções. Por exercício, avalie:
- a clareza do enunciado;
- se o formato de saída está especificado ou só implícito;
- se o teste público verifica o que o grader privado exige (liste os critérios privados sem equivalente público);
- ambiguidades que forçam adivinhação;
- testes instáveis: rode o grader 3 vezes na referência e nos 5 exercícios com mais discordância entre configurações;
- se a solução de referência passa no próprio grader (fase 0).

Use também o sinal empírico: critérios que nenhuma configuração passou e critérios que falham só por formato. Termine com uma classificação (claro, ambíguo, defeituoso) e sugestões de correção por exercício.

### Entregáveis em `WORKDIR/relatorio/`
1. `relatorio.md` e `relatorio.txt` com toda a análise e todos os números. Primeira seção: resumo de uma página com o ranking, a cadeia de ablação (direto → `fh-nomem` → `fh-mem1` → `fh-memmax`), N_MAX, os resultados dos benchmarks públicos (fase 7), a capacidade máxima medida na fase 8 e a lista do que ficou inviável neste hardware, UNSUPPORTED ou PENDENTE, com a evidência.
2. `relatorio.pdf` com tabelas e gráficos: ranking com intervalos, tokens, tempo e velocidade por configuração, aprovação por trilha/stack/nível, tok/s por fluxo e agregado contra G (calibração e mistura), e a cadeia de ablação com a diferença de cada configuração para o `qwen27b-direto`.
3. `results.csv` e `results.json` por exercício e configuração, os logs do proxy, do `/metrics` e do `nvidia-smi`, e os JSONs do grader.
4. `MANIFEST.json`:
   - commits (jg-eng-tests e `fh`);
   - versões (vLLM, CUDA, driver, runtime de contêineres e se é root ou rootless, cada harness, LiteLLM se usado, .NET, Node, pwsh, language servers, digest da imagem);
   - o arquivo de configuração de cada configuração do `fh` e o comando de cada outra;
   - o comando exato do servidor e as flags de fallback usadas;
   - amostragem imposta, `max-model-len`, `max-num-seqs` final, N_MAX, a escala de G usada e a cota do proxy por configuração;
   - as variantes de servidor da fase 8, cada uma com o comando exato;
   - para cada benchmark público: versão/commit do conjunto de tarefas, versão do Harbor/Pier, lista exata de tarefas rodadas e o motivo de cada tarefa não rodada;
   - método de identificação por configuração, regra de firewall;
   - horários de início e fim de cada fase e de cada configuração.

## 8. Fase 7: benchmark público DeepSWE v1.1 (todas as configurações em paralelo, na ordem da decisão 7)
Objetivo: rodar um benchmark público de código, o DeepSWE v1.1, com as onze configurações, como no jg-eng-tests, para comparar com o resto do mercado. Valem as mesmas regras: decisões 1, 2, 6, 7, 10 e 13, proxy de medição, amostragem imposta, isolamento entre configurações (regra 10), rede bloqueada fora do endpoint. Nada desta fase roda antes de as fases 0 a 6 terminarem. **Terminal-Bench 4.0, CWE-bench v1, Vibe Code Bench e FrontierSWE v2 não fazem parte do plano** (decisão 12).

| Benchmark | Tarefas | Runner | Onde estão as tarefas | Observação |
|---|---|---|---|---|
| DeepSWE v1.1 | 113 | Pier (fork do Harbor da Datacurve) | `github.com/datacurve-ai/deep-swe` (`tasks/`) | sem internet; nota pelo que for **commitado** (`git diff base..HEAD`) |

### 8.0 Mecânica comum
- **Uma invocação do Pier por configuração, todas ao mesmo tempo.** Cada configuração roda numa invocação própria do Pier. Nas tarefas longas o limite do paralelismo é o **host**, não o N_MAX (elas geram poucos tokens por minuto): `-n` por configuração = `floor(min(núcleos livres ÷ CPUs por tarefa, RAM livre ÷ memória por tarefa) ÷ 8 configurações)`, no mínimo 1. A cota do proxy (seção 3 item 8) e a guarda de 30 tok/s por tarefa da seção 5.0a continuam valendo na fase inteira: se a guarda falhar, reduza o `-n` de todas igualmente e registre. Como os runners não se coordenam, as invocações são lançadas juntas e o total é a soma dos `-n`. Se nem o mínimo (1 × 8 = 8 tarefas simultâneas) couber no host, rode as configurações em ondas, com as do `fh` na primeira, e registre.
- **Ordem (decisão 7).** Na fila e nos relatórios a ordem é a da tabela da seção 5: `fh-nomem`, `fh-mem1`, `fh-memmax`, `qwen27b-direto`, `opencode`, `qwen-code`, `claude-code`, `deepseek`. O DeepSWE roda num vLLM novo (regra 4).
- **Todas as tarefas, para todas as configurações (decisão 14).** As 113 tarefas do DeepSWE. Nada de subconjunto: todas as configurações rodam exatamente as mesmas tarefas, e a lista (com a versão do conjunto) vai para o `MANIFEST.json`.
- **Timeouts.** Use os timeouts oficiais de cada benchmark, sem multiplicadores. Se for preciso encurtar, isso vira variante com nome próprio, fora da comparação.
- **Piloto.** Antes de cada benchmark, 2 tarefas por configuração, **rodando juntas**, fora da contagem, para validar a ligação e o isolamento: o agente instalou, falou com o proxy usando a chave certa, o verificador rodou e gravou a nota.
- **Nota.** Vale a nota do verificador oficial de cada benchmark, sobre a entrega final (decisão 1). Faça também uma revisão às cegas, pela LLM operadora, de 10% das tarefas aprovadas de cada configuração, procurando trapaça (teste alterado, resposta fixa). Uma aprovação derrubada é reportada à parte; ela não muda a nota oficial.
- **Memória do `fh` (decisão 2).** Cada configuração do `fh` tem o seu diretório de skills por benchmark, vazio no início, passado com `--ak state_dir=WORKDIR/fh-state/<benchmark>/<configuracao>`. O adaptador copia a memória para cada tarefa e a funde de volta ao final, com trava, mesmo com tarefas em paralelo (seção 5.1: um só dono). Os diretórios das três configurações do `fh` nunca se misturam.
- **Medição.** Tudo passa pelo proxy (seção 3): tokens, TTFT, tok/s, aceitação do MTP, GPU. Some a isso o resultado do Harbor/Pier (`result.json` de cada job). Como as configurações dividem a GPU, a velocidade comparável vem de um passe de tempo (seção 5.0c): feito no DeepSWE (10 tarefas de semente fixa).

### 8.1 Como o `fh` entra nos benchmarks (adaptadores do repositório)
O repositório já traz os adaptadores em `integrations/harbor/fh_harbor/`, documentados em `integrations/harbor/README.md`:
- `fh_harbor.agent:FrankensteinHarness` para o Harbor (qualquer dataset do Harbor, e o caminho de reserva do Qwen Code e do DeepSeek no DeepSWE);
- `fh_harbor.pier_agent:FrankensteinHarness` para o Pier (DeepSWE).

Eles instalam um binário Linux estático do `fh` dentro do contêiner de cada tarefa e devolvem tokens e veredito ao Harbor/Pier. Pelas opções `--ak`, o mesmo adaptador roda as quatro configurações que são do `fh`: com `mode=run` (padrão) ele roda `fh run --auto --yes --mode yolo --json`; com `mode=direct` roda `fh direct`, o modelo sozinho.

O que foi verificado antes de entregar este plano (com o modelo simulado do `fh`, em Docker):
- **Harbor:** uma tarefa real teve nota 1,0 do verificador oficial. Três tarefas em paralelo tiveram 1,0 cada e a memória compartilhada fundiu as três.
- **Pier:** a instalação durante o build funcionou e o `fh` rodou sem internet. A chamada ao modelo foi barrada porque o proxy do Pier só deixa sair pelas portas 80 e 443 (veja 8.2). O caminho completo no Pier **não** foi confirmado; confirme no piloto.

Preparação, uma vez:
1. Compile o binário estático: `rustup target add x86_64-unknown-linux-musl`, instale `musl-tools`, e rode `CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release --target x86_64-unknown-linux-musl`. Confira com `file` que o binário é estático.
2. Instale Harbor e Pier em versões fixas, cada um no seu ambiente Python 3.12 (`uv tool install harbor`, `uv tool install datacurve-pier`), e registre as versões. Exporte `PYTHONPATH=<repo>/integrations/harbor`.
3. **Endpoint visível dos contêineres.** Os agentes rodam dentro de contêineres (Docker ou Podman, seção 1 item 9), então o proxy de medição precisa escutar num endereço que eles alcancem: o gateway do runtime de contêineres (`docker network inspect bridge`, ou `podman network inspect podman`, em geral `172.17.0.1`/`10.88.0.1`). Use `FH_ENDPOINT=http://host.docker.internal:8001/v1` com um overlay de compose (`extra_hosts: ["host.docker.internal:host-gateway"]`, passado com `--extra-docker-compose`) e `--allow-agent-host host.docker.internal`. Mantenha o firewall da seção 3: só o proxy é alcançável.
4. Os outros harnesses usam os agentes nativos do Harbor (`--agent opencode`, `--agent qwen-code`, `--agent claude-code`; confira com `harbor run --help` e `harbor agent schema <nome>`), configurados para o mesmo proxy e o mesmo `frankenstein-v2`, como nas seções 5.3 a 5.5. O DeepSeek Harness não tem agente nativo: se não houver como plugá-lo (adaptador próprio aceitando endpoint OpenAI-compatível), marque UNSUPPORTED com a evidência.

Opções do adaptador (`--ak`) de cada configuração que é do `fh`, para o Harbor e para o Pier:

| configuração | opções |
|---|---|
| `fh-nomem` | `--ak max_concurrency=1 --ak memory=off --ak state_dir=WORKDIR/fh-state/<benchmark>/fh-nomem` |
| `fh-mem1` | `--ak max_concurrency=1 --ak state_dir=WORKDIR/fh-state/<benchmark>/fh-mem1` |
| `fh-memmax` | `--ak max_concurrency=16 --ak consolidate=true --ak state_dir=WORKDIR/fh-state/<benchmark>/fh-memmax` |
| `qwen27b-direto` | `--ak mode=direct` (e `--ak commit=true` no DeepSWE) |

As outras quatro usam os agentes nativos (seção 5.3 a 5.6), apontados para o mesmo proxy, cada uma com a sua chave.

### 8.2 DeepSWE v1.1 (Pier)
- Clone `datacurve-ai/deep-swe`, confirme que as tarefas são da v1.1 (README, `task.toml`, imagem `...-v1.1`) e registre o commit.
- **Binário por URL.** O Pier embute o agente na imagem durante o build, então o binário precisa vir por URL. Sirva-o de uma pasta do host: `python3 -m http.server 8090 --bind 172.17.0.1`, e passe `FH_BINARY_URL=http://172.17.0.1:8090/fh` mais `FH_BINARY_SHA256`.
- **Endpoint na porta 80 ou 443.** Na execução, a única saída é o proxy Squid do Pier, que só aceita essas portas. Ponha uma segunda escuta do proxy de medição em `172.17.0.1:80` (porta abaixo de 1024: precisa de root ou do `sysctl` da seção 1 item 9) e use `FH_ENDPOINT=http://172.17.0.1/v1`. Confirme no piloto que o `fh` recebe resposta.
- **Commit obrigatório.** A nota sai de `git diff base..HEAD`, então use `--ak commit=true`: o `fh` commita quando a verificação passa ou quando não havia como verificar.

```bash
pier run -p deep-swe/tasks \
  --agent-import-path fh_harbor.pier_agent:FrankensteinHarness -m openai/frankenstein-v2 \
  --ae FH_ENDPOINT=http://172.17.0.1/v1 --ae FH_BINARY_URL=http://172.17.0.1:8090/fh \
  --ak commit=true --ak max_concurrency=1 --ak memory=off --ak state_dir=WORKDIR/fh-state/deepswe/fh-nomem \
  -n <o -n da seção 8.0> -o WORKDIR/bench/deepswe/fh-nomem
```
- **Exemplo acima: `fh-nomem`.** As outras configurações do `fh` mudam só as opções da tabela da seção 8.1, e o `qwen27b-direto` usa `--ak mode=direct --ak commit=true`.
- **Outros harnesses.** O Pier tem OpenCode e Claude Code nativos. Para o Qwen Code e o DeepSeek, que não estão no Pier, tente as mesmas tarefas no Harbor. Se o Harbor não suportar o formato 1.3 dessas tarefas (`verifier.collect`, `environment_mode = "separate"`), porte o adaptador do Harbor (`qwen_code.py`, ou o do DeepSeek) para o Pier, que tem a mesma ideia com `install_spec` (como o `fh_harbor.pier_agent` do repositório). Só se isso for impossível, marque UNSUPPORTED com a evidência e avise o dono.
- A Epoch apontou problemas em pelo menos 23 das 113 tarefas. Se houver lista pública delas, reporte também a nota sem essas tarefas.
- Referência externa: o líder público tinha 75,4%, e a faixa dos nove modelos da v1.1 ia de 12% a 70%.

### 8.3 Relatório do benchmark
Para cada configuração, reporte:
- nota oficial com intervalo de Wilson de 95% (ou média com intervalo por bootstrap, quando a nota for contínua);
- **a diferença para o `qwen27b-direto`** (melhorou, piorou ou sem diferença conclusiva) e a cadeia de ablação da seção 7 (harness, memória, paralelização) também nesses benchmarks;
- comparação pareada tarefa a tarefa contra o `fh-mem1` e o `fh-memmax`;
- tokens (total e sem cache) por tarefa resolvida e o `-n` usado; tempo por tarefa só do passe de tempo, quando houve;
- tarefas inviáveis neste hardware ou UNSUPPORTED, com a evidência;
- a referência externa do placar, marcada como ambiente diferente (outro modelo e, às vezes, outro harness).

Inclua tudo em `relatorio.md`, no PDF e em `results.csv` (coluna `benchmark`).

## 9. Fase 8: teste de capacidade do `fh` em uso extremo (por último, depois de tudo)
Objetivo (decisão 5): quantos usuários simultâneos o servidor aguenta com o `fh`, e com que tok/s por usuário. Isso fica fora da comparação entre configurações, então aqui **é permitido** testar variantes de servidor, cada uma com nome próprio e servidor reiniciado. **A máquina fica só para este teste:** nenhuma outra configuração, benchmark ou processo pode estar rodando (confira com `nvidia-smi` e `docker ps` ou `podman ps` antes de cada degrau).

**Definições.**
- **Usuário** = uma sessão do `fh` resolvendo um exercício. Cada usuário recebe uma cópia nova de um exercício, percorrendo o catálogo em ciclo. Use `fh-mem1` (um agente, com memória) como carga principal, e depois repita os degraus principais com o `fh-memmax` (paralelização máxima, com consolidação), cuja carga por usuário é maior. A memória de cada usuário parte de uma cópia vazia (seção 5.1: um só dono).
- **tok/s de uma atividade** = tokens de saída de uma requisição ÷ (duração − TTFT), do log do proxy.
- **tok/s por usuário:**
  - durante a geração: média do tok/s das requisições daquele usuário, ponderada pelos tokens;
  - efetivo: saída total da sessão ÷ tempo de parede da sessão, que inclui o tempo de ferramentas e de build.

  Reporte os dois: o primeiro mede o servidor, o segundo o que o usuário sente.
- **tok/s médio por usuário no degrau** = média e mediana entre os usuários do degrau, com P10 e P90.

**Técnica 1: carga real com agentes (principal).**
- Servidor de referência (a mesma configuração da comparação) recém-iniciado, com a memória do `fh` vazia.
- **Degraus:** começa com **20 usuários ao mesmo tempo**, todos lançados juntos. Depois: 30, 40, 60, 80, 120, 160, e segue aumentando 50% por degrau enquanto não parar (menos degraus que a escada fina, o mesmo resultado).
- Cada degrau dura pelo menos 8 minutos em carga constante: quando um usuário termina, outro entra na hora.
- Todos os exercícios concluídos são corrigidos pelo grader, para ver se a qualidade cai sob carga.
- Grave por degrau:
  - usuários ativos;
  - tok/s por usuário (os dois tipos) com média, mediana, P10 e P90;
  - tok/s agregado do servidor;
  - TTFT P50/P90/P99, fila (rodando e esperando), KV cache e preempções, aceitação do MTP;
  - CPU e RAM do host, GPU, potência e temperatura;
  - erros, timeouts de 900 s, taxa de aprovação no grader.
- **Pare de subir** quando acontecer qualquer um destes:
  - taxa de erro acima de 1%;
  - falta de memória ou queda do servidor;
  - tok/s mediano por usuário (durante a geração) abaixo de 5;
  - TTFT P90 acima de 60 s;
  - timeouts acima de 25%;
  - GPU acima de 90 °C por mais de 1 minuto.

  O último degrau cumprido é o limite.
- **Capacidade em três níveis**, cada um com o N e os números:
  1. **Confortável:** tok/s mediano por usuário ≥ 20, P10 ≥ 10, TTFT P90 ≤ 10 s, zero erros e aprovação sem queda significativa em relação à comparação principal (teste binomial).
  2. **Aceitável:** tok/s mediano por usuário ≥ 10, TTFT P90 ≤ 30 s, erros < 1%, timeouts < 10%.
  3. **Limite:** o último degrau antes do critério de parada.

  Se 20 já não for confortável, desça em degraus de 4 (16, 12, 8) até achar o confortável. Depois de achar o limite, refine **uma vez** entre o último degrau bom e o primeiro ruim (ponto médio), e pare.

**Técnica 2: repetição das sessões gravadas (isola o servidor).**
- Com os logs do proxy da comparação principal (requisições completas do `fh`, com os intervalos reais entre elas), escreva um reprodutor que dispara N sessões gravadas ao mesmo tempo. Use só 4 degraus: metade do limite da técnica 1, o limite, o dobro e o quádruplo (até 256 ou até quebrar), com 5 minutos cada.
- Mande os mesmos prompts com os tempos originais entre chamadas, sem executar ferramentas nem builds. Isso tira a CPU do caminho e mostra o limite só do servidor.
- A diferença entre o limite das técnicas 1 e 2 mostra quanto do limite vem do host (builds) e quanto vem da GPU.

**Técnica 3: carga sintética padronizada.**
- Use `vllm bench serve` da versão instalada (confira as opções em `--help`), com distribuições de tamanho de entrada e saída iguais às medidas nos logs, prefixo compartilhado igual ao observado, e `--max-concurrency` em 1, 16, 64 e 256, 2 minutos cada.
- Faça também uma varredura curta por taxa de chegada (`--request-rate`, 4 taxas, 2 minutos cada) até saturar, para encontrar a vazão máxima sustentável em req/s e tok/s.
- Esses números são comparáveis com outros servidores e outras GPUs.

**Técnica 4: pico súbito.** De 0 para o limite "aceitável" de uma vez. Meça quanto tempo leva até o TTFT estabilizar e se há erros ou preempções no pico.

**Técnica 5: resistência.** 20 minutos contínuos no nível "confortável". Procure degradação ao longo do tempo: tok/s caindo, VRAM ou RAM subindo, temperatura, erros tardios.

**Variantes de servidor, todas obrigatórias** (cada uma reiniciada e medida com a técnica 3 e, nos 3 degraus ao redor do limite, com a técnica 1), nesta ordem:
1. `max-num-seqs` 128;
2. MTP com `num_speculative_tokens` 1 e, em seguida, sem MTP (sob lote grande, a especulação pode render menos que o custo);
3. `max-model-len` 65536 (mais sequências cabem no KV cache).
4. `max-num-seqs` 256;
5. `gpu-memory-utilization` 0.95;
6. `max-num-batched-tokens` maior e depois menor, com o chunked prefill da versão instalada.

Confira cada flag em `vllm serve --help`. Registre para cada variante a capacidade confortável e o limite, e diga qual configuração maximiza usuários confortáveis e qual maximiza a vazão agregada. Podem não ser a mesma.

**Entregável da fase 8:** `WORKDIR/relatorio/capacidade.md` e seção própria no PDF, com:
- gráficos de tok/s por usuário (mediana e P10) e tok/s agregado contra usuários simultâneos, para cada técnica e variante;
- a tabela de capacidade em três níveis;
- a recomendação final: quantos usuários simultâneos do `fh` a GPU aguenta com conforto, com qual configuração e com que tok/s médio por usuário.

Se bater um limite de disco ou de hardware, **pare e pergunte ao dono**; não corte em silêncio. O relatório parcial sai sempre, com o que já foi medido, e o que ainda falta fica como PENDENTE (nunca como pulado) até o dono decidir. No fim, o que restar de inviável neste hardware ou UNSUPPORTED aparece com a evidência de cada tentativa.

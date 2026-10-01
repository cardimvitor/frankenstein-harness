# Prompt de execução V2: validação de harnesses com o Frankenstein V2 (jg-eng-tests)

Cole este documento inteiro numa sessão do Claude Code com terminal na máquina da GPU. Execute, não apenas descreva. Não invente resultados: o que não for medido fica NOT_RUN ou N/D, com o motivo.

Esta versão substitui a primeira (`docs/VALIDACAO_JG_ENG_TESTS.md`) e incorpora as decisões abaixo.

## Decisões do dono do projeto (valem acima de qualquer outra regra deste documento)
1. **Só vale o resultado final.** Se o `fh` errou duas vezes e acertou na terceira, antes de entregar, conta como certo, sem penalidade. Tentativas intermediárias, rodadas de verificação e autocorreções são o processo interno do harness e não entram na nota nem no relatório como erro. Corrija o que ele deixar no disco ao terminar, sem `--keep`. Se ele desfizer a própria mudança, a entrega é o estado original. O mesmo vale para todos os harnesses.
2. **O `fh` pode aprender entre exercícios.** A memória de skills é parte do harness e está sendo validada junto. O estado dele persiste de um exercício para o outro dentro do mesmo modo, e qualquer outro harness com memória persistente nativa recebe o mesmo tratamento.
3. **Resolver antes de rodar e verificar.** Tudo que pode quebrar por ambiente ou configuração é resolvido e verificado antes da execução que conta. A falha de verificação do `fh` que a primeira versão apontava (não conhecer `scripts/test.sh`) já foi corrigida no próprio `fh` (seção 5.1).
4. **Concorrência na primeira rodada.** Nada de ir direto a 20. Sobe até 5 exercícios em paralelo. Depois valida, degrau a degrau, se chega a 10 sem prejudicar o desempenho, e só sobe um degrau depois de confirmar que o anterior aguentou.
5. **Teste final de capacidade, só do `fh`.** Ao final de tudo, começa com 20 sessões simultâneas e vai aumentando até onde o servidor aguenta. Mede o tok/s de cada atividade para calcular o tok/s médio por usuário e descobre a capacidade máxima em uso extremo, com outras técnicas de carga além da carga real (seção 9).
6. **Quem julga a assertividade final é a LLM que conduz os testes, não o Qwen.** O juiz é você, a LLM operadora que está executando este prompt. O Frankenstein V2 (Qwen) aparece só como o modelo avaliado, dentro dos harnesses. Nada do que ele diz sobre o próprio trabalho entra na nota: nem o veredito interno do `fh`, nem o revisor LLM do `fh`, nem o "terminei, todos os testes passam" de qualquer harness. Esses sinais não entram no relatório como métrica. Detalhes na seção 5.0b.
7. **O `fh` vem sempre primeiro, nas duas versões.** Em cada comparação (jg-eng-tests e cada benchmark público), a ordem é: `fh` modo 1 agente, `fh` modo máximo, e só depois os outros harnesses. Os benchmarks públicos (seção 8) rodam para todos os harnesses, como o jg-eng-tests, com tarefas em paralelo.
8. **A melhor configuração possível para uma RTX PRO 6000 Blackwell.** Antes de qualquer teste, baixe e compare os checkpoints oficiais do Qwen3.8-27B próprios para essa placa (NVFP4 nativo do Blackwell, FP8, e BF16 como referência de qualidade), ajuste o vLLM e fixe o vencedor para a sessão inteira (seção 2.0).
9. **A máquina inteira é da avaliação.** Antes de qualquer teste, libere toda a VRAM e os demais recursos parando os processos que não fazem parte da avaliação, e baixe e instale tudo de uma vez, em paralelo (seção 1, itens 7 e 8).

Nomes usados aqui: **Frankenstein V2** é o modelo (Qwen3.8-27B NVFP4 + MTP=3 no vLLM). **Frankenstein Harness** (`fh`) é o nosso harness. Não confunda os dois no relatório.

## CONFIGURAÇÃO (preencher antes de colar)
- REPO: https://github.com/dfnb/jg-eng-tests, fixado no commit `36cbe4741e5730fae876fae3bff6fa167fb18cb7`. Se o HEAD for outro, pare e avise.
- GPU: uma RTX PRO 6000 Blackwell 96 GB (SM120), inteira para a avaliação (decisão 9). O checkpoint e as flags saem da seção 2.0
- WORKDIR: `<ex.: /workspace/harness-eval>` (precisa de ~150 GB livres para as execuções, mais ~200 GB para os checkpoints candidatos da seção 2.0 e as imagens Docker dos benchmarks)
- FRANKENSTEIN_HARNESS: repositório https://github.com/cardimvitor/frankenstein-harness, branch `ccr-3bc51f62-prkdq0`. Compilar com `cargo build --release` e usar `target/release/fh`. Registre o commit.
- EXERCICIOS_EM_PARALELO: rampa gradual 2 → 3 → 4 → 5; depois 6 → 7 → 8 → 9 → 10, um degrau por vez e só se o anterior não prejudicou o desempenho. O teto N_MAX sai da calibração e vale no máximo 10 nesta rodada (seção 5.0a). A escala é a mesma para todos os harnesses
- TIMEOUT_POR_EXERCICIO: 900 s de parede, contando do início do harness até o processo terminar
- REPETICOES: 1 (3 se houver tempo, só para os 2 melhores harnesses)
- AMOSTRAGEM: temperatura 1,0, top_p 0,95, top_k 20, igual para todos e imposta pelo proxy (seção 2)
- EXECUTAR_ATE: fase `<0 a 8>` (a 7 são os benchmarks públicos; a 8, o teste de capacidade do `fh`, sempre por último)

Estimativa de tempo, para planejar: com N_MAX = 10, o pior caso (todos os exercícios estourando o timeout) fica em ~3,5 h por harness, ou ~21 h para as 6 execuções, mais a calibração (~2 h) e o teste de capacidade (~4–8 h). Os benchmarks públicos (fase 7) são bem mais longos (tarefas de 8 h no Terminal-Bench, até 20 h no FrontierSWE): planeje dias, use um subconjunto fixo se precisar e retome pelo `state.json`. O normal fica bem abaixo disso. A sessão precisa conseguir retomar sem refazer o que já terminou (seção 4.0).

## 0. Regras que valem a sessão inteira
1. Os agentes avaliados só veem a pasta `exercise/` de cada desafio. Nunca `solution/`, `grader/`, `EVALUATION.md`, o `.git` do jg-eng-tests, as pastas de outros exercícios ou resultados de outros harnesses. A única exceção é a memória interna do próprio harness (decisão 2): o que ele aprendeu sozinho nos exercícios anteriores, nunca a nota do grader. Você (operador) nunca resolve exercício, nunca dá dica e nunca cola conteúdo do grader em prompt.
2. Um modelo só: o Frankenstein V2 servido pelo nosso vLLM. Nenhum harness pode chamar modelo externo. Bloqueie a saída de rede dos agentes (só o proxy local é alcançável) e prove pelos logs do proxy e do firewall que 100% das chamadas de modelo foram para o vLLM local.
3. Mesmo prompt de tarefa para todos os harnesses, mesmo timeout, a mesma escala de concorrência de exercícios (a rampa da seção 5.0a, amarrada à posição do exercício no catálogo), mesma amostragem, mesma ordem de exercícios (a do `catalog.json`).
4. Cada harness roda num vLLM recém-iniciado (processo novo, cache vazio). O mesmo vale entre os dois modos do Frankenstein Harness. A memória do harness começa vazia no início de cada harness e de cada modo.
5. Não mude a configuração do servidor entre harnesses. Se algo precisar mudar, rode como variante separada, com nome próprio, fora da comparação principal.
6. Não grave chaves ou tokens em arquivo, log ou relatório. Nos logs, identifique chaves pelo SHA-256 truncado.
7. Não altere nenhum arquivo do jg-eng-tests, nem os graders. Correções sugeridas vão só para o relatório.
8. O julgamento final (seção 5.0b) é feito só pela LLM operadora, com o grader oficial como base objetiva. É proibido usar o vLLM/Qwen, ou qualquer harness avaliado, para julgar, resumir ou classificar resultados. O operador também não usa o julgamento para ajudar os agentes: ele só acontece depois que o exercício terminou.
9. Um erro ou resultado estranho do nosso harness (`fh`) é um achado, não algo a esconder. Registre com evidência. Problemas encontrados *antes* da execução que conta (fase 0, pilotos) são corrigidos antes de começar (decisão 3): registre o commit do `fh` usado. Não corrija o `fh` no meio da comparação, porque isso invalidaria a execução. Se a correção for indispensável, termine a execução, corrija, e rode de novo como variante com nome próprio.

## 1. Fase 0: preparação e sanidade (antes de qualquer harness)
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
3. Imagem de execução única, a mesma para todos os harnesses (Docker ou Podman). Deve conter:
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
   1. **Inventário antes de mexer.** Grave em `WORKDIR/maquina/antes.txt`: `nvidia-smi`, `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv`, `ps aux --sort=-%mem | head -40`, `ps aux --sort=-%cpu | head -40`, `systemctl list-units --type=service --state=running`, `docker ps -a`, `df -h`, `free -h`, `lscpu`, `who`, `uptime`.
   2. **Libere a GPU.** Pare tudo que usa a placa: vLLM ou Ollama antigos, notebooks, treinos, contêineres com `--gpus`, outros servidores de modelo. Primeiro do jeito educado (`systemctl stop`, `docker stop`, `SIGTERM` e espera de 10 s), depois `SIGKILL` no que sobrar. Impeça que voltem durante a sessão (`systemctl disable --now`, `docker update --restart=no <contêiner>`, ou `systemctl mask` temporário). Alvo: `nvidia-smi` sem nenhum processo de computação e com menos de 500 MiB de VRAM usada. Se a VRAM continuar ocupada sem processo dono, tente `fuser -v /dev/nvidia*` e `nvidia-smi --gpu-reset`; se ainda assim não liberar, pare e avise o dono (pode exigir reiniciar a máquina).
   3. **Libere CPU, RAM e disco.** Pare os processos que consomem CPU ou RAM sem fazer parte da avaliação. Libere disco com `docker container prune`, `docker builder prune` e `docker image prune` (só imagens sem uso e sem nome). Não apague volumes, bancos de dados nem arquivos de pessoas.
   4. **Nunca mate, em nenhuma hipótese:** a sua própria sessão e o shell em que você roda (inclusive `tmux`/`screen`), o `sshd` e as conexões SSH ativas, `systemd`/`init`, `dockerd`/`containerd`, a rede (`NetworkManager`, `systemd-networkd`, resolvedor de nomes), o driver da NVIDIA e o `nvidia-persistenced`.
   5. **Pergunte antes de parar:** processo de outro usuário com sessão interativa ativa, e qualquer serviço que pareça de produção ou guarde dados de outras pessoas (banco de dados, servidor web, fila). Se for ambíguo, pergunte; não presuma.
   6. **Registre tudo.** `WORKDIR/maquina/parado.md`: o que foi parado, o dono, o comando e o comando para restaurar. Ao final da sessão, ofereça restaurar.
   7. **Desempenho (precisa de root; sem root, anote que não deu):** `nvidia-smi -pm 1` (modo persistente), governador de CPU em `performance` (`cpupower frequency-set -g performance`), `ulimit -n` alto, `/dev/shm` com pelo menos 16 GB (e `--shm-size` nos contêineres) e swappiness baixo. Grave os valores em `WORKDIR/maquina/depois.txt`, junto de um segundo `nvidia-smi` mostrando a VRAM livre.
   8. **Reserva.** O vLLM e o proxy têm prioridade. Deixe sempre livres pelo menos 4 núcleos e 16 GB de RAM para eles, e limite o paralelismo das tarefas de acordo (as `cpus` e `memory_mb` de cada tarefa são o teto; reduza `-n` antes de tirar recurso do vLLM).
8. **Baixe e instale tudo logo no começo, em paralelo.** Primeiro confira o espaço em disco contra a soma do que vai baixar (checkpoints, imagens Docker, datasets, caches); se faltar, pare e avise. Depois dispare, em paralelo e em segundo plano, cada item com seu log em `WORKDIR/instalacao/<item>.log`, com no máximo 4 a 6 downloads simultâneos, para não saturar disco e rede:
   - os checkpoints candidatos da seção 2.0 (`hf download` com `HF_HUB_ENABLE_HF_TRANSFER=1`);
   - vLLM em versão fixa, ou a sua imagem Docker, e o driver/CUDA que a receita pede;
   - imagens Docker: as bases dos exercícios, as dos benchmarks (Terminal-Bench, DeepSWE, FrontierSWE), as do proxy e do LiteLLM;
   - toolchains: Rust (e o alvo musl do `fh`), .NET SDK 10.0.401, Node 24.15.0, `pwsh`, Python 3.12 com `uv`, e os language servers;
   - os harnesses: OpenCode, Qwen Code, Claude Code, DeepSeek Harness, LiteLLM;
   - Harbor e Pier em versões fixas, e os datasets e repositórios: `jg-eng-tests`, `datacurve-ai/deep-swe`, `Proximal-Labs/frontier-swe-v2` (cerca de 770 MB), `vals-ai/VibeCodeBench-Openhands-Scaffold`, e `harbor download` dos datasets do Harbor Hub;
   - os caches de pacotes dos exercícios (`dotnet restore`, `npm ci`), já que depois os agentes rodam sem rede.

   Compile o `fh` (nativo e estático) enquanto os downloads rodam. Ao terminar, verifique tudo (checksums dos checkpoints, `--version` de cada ferramenta) e grave `WORKDIR/instalacao/VERSOES.md`. O que falhou é repetido uma vez; se falhar de novo, vai para o `REVISAO.md`. Nada pode depender de internet depois desta etapa, exceto as chamadas de juízes externos do CWE-bench e do Vibe Code Bench, se o dono liberar.

## 2. Fase 1: servidor, configuração validada do Frankenstein V2

### 2.0 Escolha do checkpoint e da configuração para uma RTX PRO 6000 Blackwell (decisão 8, antes de tudo)
O dono do projeto quer a melhor configuração possível para **uma** RTX PRO 6000 Blackwell (96 GB, SM120). O que sair daqui vira o "Frankenstein V2" da sessão inteira e fica **fixo** em todas as fases seguintes (regra 5). A configuração validada antes, logo abaixo, é a linha de base que os outros candidatos precisam bater.

**Candidatos.** Só pesos oficiais do Qwen3.8-27B ou quantizações fiéis deles, todos com a cabeça MTP do próprio modelo. Nada de fine-tunes, "uncensored", merges ou destilações da comunidade.
1. `nvidia/Qwen3.8-27B-NVFP4`, a linha de base validada: revisão `dbb8f445`, 122,1 tok/s por fluxo e aceitação do MTP de 75,7%.
2. `nvidia/Qwen3.8-27B-NVFP4` na revisão mais recente, se for diferente de `dbb8f445`.
3. `Inferact/Qwen3.8-27B-NVFP4`, citado com aceitação do MTP de 0,897 em outra GPU.
4. `unsloth/Qwen3.8-27B-NVFP4`, que mantém módulos sensíveis em 8 bits.
5. FP8 oficial do Qwen, se existir (`Qwen/Qwen3.8-27B-FP8`), como meio-termo entre qualidade e velocidade.
6. `Qwen/Qwen3.8-27B` em BF16 (~54 GB de pesos), como **referência de qualidade**: cabe na placa, mas sobra pouco KV cache.

Confira no Hugging Face se cada um existe, a revisão, a licença e o model card; registre o hash de cada download. Consulte a receita oficial do vLLM para essa placa (`recipes.vllm.ai/Qwen/Qwen3.8-27B`, alvo RTX Pro 6000) e o model card de cada checkpoint, e use as flags que eles indicam: nome do método MTP, quantização, parsers.

**Configuração a ajustar, com o candidato fixo:**
- MTP `num_speculative_tokens` em 1, 2, 3 e 4;
- `kv-cache-dtype` `fp8` contra `auto`;
- `gpu-memory-utilization` entre 0,85 e 0,95 (a RTX 6000 não tem a memória unificada da DGX Spark; como a GPU está livre de outros processos desde o item 7 da fase 0, pode subir enquanto estável; se faltar memória, desça);
- `max-num-batched-tokens` e chunked prefill;
- `max-num-seqs` 32 e 64;
- `max-model-len` 131072, ou 262144 se couber com KV suficiente para N_MAX sessões;
- a correção do SM120 (`--quantization modelopt_fp4 --block-size 128` com `VLLM_HAS_FLASHINFER_CUBIN=1`) só se o padrão travar.

Use a versão do vLLM mais recente estável que a receita indica (no mínimo 0.29.0) com CUDA 13.x, e registre o driver.

**Como medir.** Nada daqui usa os exercícios do jg-eng-tests nem tarefas dos benchmarks públicos, para não contaminar a comparação.
- **Qualidade:**
  - `fh validate-vllm` (tool calls, vazamento de raciocínio, contexto longo);
  - o corpus embutido do `fh` (`fh eval --tasks` com as tarefas do repositório);
  - um subconjunto fixo de 40 tarefas do Aider polyglot (`scripts/polyglot_to_tasks.py`, semente registrada), com o `fh` no modo 1 agente.
- **Velocidade:** tok/s por fluxo com 1 sessão, tok/s agregado com 8 sessões, TTFT P50/P90, aceitação do MTP e KV cache disponível, medidos com `vllm bench serve` usando entradas e saídas de tamanho parecido com o de agentes (prompt de 20–60k tokens com prefixo compartilhado, saída de 500–2000).

**Regra de escolha.**
1. Elimine o candidato cuja aprovação no Aider polyglot ficar abaixo da BF16 com significância (McNemar exato, p < 0,05), ou que errar tool calls em mais de 1% das chamadas.
2. Entre os que sobram, escolha o de maior tok/s agregado com 8 sessões, desde que o tok/s por fluxo com 1 sessão não caia mais de 10% em relação ao melhor.
3. Com o checkpoint escolhido, fixe a combinação de flags com maior tok/s agregado que não aumente erros nem diminua a aprovação.

Grave tudo em `relatorio/escolha_modelo.md`: tabela de candidatos × qualidade × velocidade, a escolha e o motivo. O comando exato vencedor substitui o comando abaixo em todas as fases e vai para o `MANIFEST.json`. Se nenhum candidato bater a linha de base, use a linha de base e registre.

**Tempo-limite desta etapa:** cerca de 6 h. Se não der para medir todos os candidatos, priorize 1, 3, 4 e 6, nessa ordem.

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
- `max-num-seqs`: 16 foi validado na PRO 5000, mas com até 10 exercícios em paralelo (e o modo máximo do `fh` abrindo vários workers por exercício), 16 vira fila na hora. Suba para 64 na PRO 6000 de 96 GB. Na calibração (seção 5.0a), confira no `/metrics` se o uso de KV cache passa de 95% ou se há preempções. Se passar, reduza para 48 ou 32 e registre. Uma vez escolhido, o valor é fixo para todos os harnesses.
- Com MTP, o ganho da especulação cai quando o lote cresce: a GPU deixa de estar ociosa entre tokens. É por isso que a rampa mede tok/s e aceitação a cada degrau. Se a calibração mostrar que acima de certo N o MTP piora o agregado, registre como achado. Não desligue o MTP na comparação principal; isso é testado como variante na fase 8.
- Reinício "limpo" = matar o processo, esperar a VRAM voltar ao repouso no `nvidia-smi`, subir de novo e esperar `GET /v1/models` responder. O cache de prefixo morre com o processo; não é preciso mais nada.
- Teste de fumaça, a cada reinício e antes de cada harness:
  1. Uma chamada simples.
  2. Uma chamada com ferramenta: confira que `tool_calls` vem estruturado e que o conteúdo não traz XML cru.
  3. Uma chamada com streaming, conferindo que `reasoning_content` vem separado.
  4. Leitura de `/metrics`, conferindo que existem as métricas de decodificação especulativa (nomes com `spec_decode`).

  Use também `fh doctor` e, só na primeira vez, `fh validate-vllm` apontados para o proxy. Guarde as saídas.

## 3. Fase 2: proxy de medição (obrigatório)
Escreva um proxy fino (Python + aiohttp ou equivalente) entre os harnesses e o vLLM, na porta 8001, repassando para a 8000. Todos os harnesses apontam para o proxy, nunca direto para o vLLM. Requisitos:

1. **Endpoints.**
   - `/v1/chat/completions`, `/v1/completions` e `/v1/models`, com e sem streaming (SSE repassado pedaço a pedaço, sem bufferizar).
   - `/v1/messages`: só se o vLLM instalado expuser; caso contrário, o tradutor da seção 5.4 fica entre o Claude Code e o proxy.
   - `GET /metrics`, repassado sem alteração (o `fh` lê a métrica para regular a concorrência).
   - Qualquer outro caminho: 404 registrado.
2. **Identificação do exercício.** Uma chave de API aleatória por par (harness, exercício), gerada pelo operador e passada ao harness. O proxy mapeia chave → (harness, exercício) numa tabela em memória carregada de um arquivo que só o operador lê. No log vai o SHA-256 truncado da chave, nunca a chave. Se algum harness não permitir chave por exercício, use o cabeçalho `x-client-id` ou a janela de tempo, e registre o método.
3. **Amostragem imposta.**
   - Em toda requisição, sobrescreva `temperature=1.0`, `top_p=0.95` e `top_k=20`, e registre os valores que o harness tinha enviado.
   - Não toque em `max_tokens`, `tools`, `tool_choice`, `chat_template_kwargs` nem `response_format`. Desligar o raciocínio em chamadas auxiliares é decisão de projeto do harness e deve aparecer no relatório; não é um desvio.
4. **Uso de tokens.**
   - Em streaming, se faltar `stream_options.include_usage`, injete-o, e remova do fluxo devolvido o pedaço extra de uso quando o cliente não o pediu.
   - Registre `prompt_tokens`, `completion_tokens` e `prompt_tokens_details.cached_tokens`.
   - O vLLM não separa tokens de raciocínio no `usage`: conte-os tokenizando o `reasoning_content` com o tokenizer do modelo e marque o campo como "estimado".
5. **Por requisição, grave em JSONL:**
   - horário de início e de fim (monotônico e de parede), harness, modo, exercício;
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
7. **Prova de isolamento de rede.** Os contêineres dos agentes ficam numa rede sem saída. Por exemplo, uma rede `--internal` do Docker em que só o contêiner do proxy (ou um encaminhador para a 8001 do host) é alcançável. Ou usuário dedicado + regra de firewall por dono (`iptables -m owner --uid-owner <uid>`) que aceita só 127.0.0.1:8001 e registra (`LOG`) e rejeita o resto. Guarde a contagem de pacotes bloqueados por harness. Tentativas bloqueadas (telemetria, atualização automática) não são erro, mas entram no relatório.

Teste o proxy antes da fase 4:
- A chamada com ferramenta, a chamada em streaming e a leitura de `/metrics` passando pelo proxy.
- Uma requisição com `temperature=0.2` precisa chegar ao vLLM com 1,0.
- Uma tentativa de `curl https://example.com` de dentro do contêiner precisa falhar.

## 4. Fase 3: isolamento dos exercícios
1. Para cada harness e cada modo, crie `WORKDIR/runs/<harness>/<exercicio>/` copiando apenas `exercise/`. Em cada cópia, e igual para todos os harnesses, rode:
   - `git init`, seguido de um commit único "base" (autor fixo). O `fh` precisa de um repositório git para checkpoints e diffs, e os outros harnesses também usam `git status`/`git diff`. Esse `.git` novo contém só o conteúdo do exercício; o `.git` do jg-eng-tests nunca é copiado.
   - `scripts/setup.sh`, **com rede** e **antes** de o agente começar (`npm ci` / `dotnet restore`), para que o agente rode sem rede. Use um cache de NuGet/npm por harness montado em modo leitura depois do setup. Se o setup falhar, é erro de infraestrutura daquele exercício, não do harness.
2. Verificação por script, que bloqueia a fase se falhar. Nenhuma cópia pode conter `solution/`, `grader/`, `EVALUATION.md`, nenhum arquivo cujo hash coincida com um arquivo de `solution/` ou `grader/` que não exista também em `exercise/`, e nenhum `.git` com mais de um commit. Guarde a saída.
3. Cada exercício roda em contêiner próprio e descartável, com a cópia montada em `/work`, que também é o diretório de trabalho. Nada mais do disco do host fica montado, fora o cache de pacotes (só leitura), a configuração do harness (só leitura) e o diretório de memória persistente do harness (decisão 2; seção 5.1 para o `fh`). Assim, "só pode ler e escrever dentro da sua pasta" vale para qualquer harness, inclusive para comandos de shell. Cada exercício é uma sessão nova, com a conversa isolada; só a memória que o próprio harness mantém passa de um exercício para o outro.
4. Os graders ficam em `WORKDIR/src/jg-eng-tests`, fora de qualquer montagem dos agentes.

Prompt de tarefa, idêntico para todos:
"Leia README.md e CHALLENGE.md desta pasta e implemente o que o desafio pede. Preserve as APIs públicas. Rode ./scripts/test.sh e ./scripts/lint.sh e corrija até passarem. Não acesse nada fora desta pasta. Ao terminar, resuma o que mudou e as suposições que fez."

## 5. Fase 4: execução, nesta ordem

### 5.0 Mecânica comum a todos
- Um orquestrador (script do operador) percorre os exercícios na ordem do `catalog.json`, com a concorrência dada pela rampa da seção 5.0a.
- Estado em `WORKDIR/state.json` com a situação de cada par (harness, exercício): pendente, rodando, terminado, erro de infra, timeout. Se a sessão cair, retome pelo estado sem refazer o que terminou; um exercício que estava "rodando" é refeito do zero, com cópia nova.
- Timeout de 900 s: mate o grupo de processos inteiro (ou o contêiner) e marque `timeout`. O que ficou no disco é corrigido normalmente; timeout conta como tentativa válida, não como erro de infraestrutura.
- Guarde stdout/stderr de cada execução, o código de saída, o `git diff` final contra o commit "base" e a última mensagem do agente.
- Correção: copie a pasta final para `WORKDIR/grading/<harness>/<exercicio>/` e rode `python3 <jg-eng-tests>/exercises/<ex>/grader/run.py <cópia>`. Aprovado = `minimumPassed == true` no JSON, que já exige todos os critérios de nível mínimo e pelo menos 60% dos pontos. Guarde o JSON inteiro: critérios, pontos e diagnósticos. O grader tem timeout interno de 90 ou 120 s; se estourar, rode de novo uma vez e registre.
- **Erro de infraestrutura** (fora do denominador, listado com evidência): o harness quebra antes da primeira chamada ao modelo; o proxy ou o vLLM devolve 5xx ou fica inacessível; falha do contêiner, falta de memória ou disco; falha do `setup.sh`. Não são erro de infraestrutura: timeout, o agente desistir, tool call malformada, o agente estourar o contexto ou terminar sem mudar nada.
- Antes de cada harness: um piloto com 3 exercícios, um .NET, um React e um Angular, por exemplo 01, 07 e o primeiro Angular do catálogo. Os pilotos entram num servidor reiniciado depois e não contam. Servem para validar a configuração: a chamada chegou ao proxy com a chave certa, o harness escreveu arquivos, rodou `test.sh`. Se o piloto revelar erro de configuração do harness, corrija a *configuração* (nunca o prompt de tarefa) e repita o piloto. Depois disso, reinicie o vLLM e rode os 130.
- Para cada item: reiniciar o vLLM, esperar ficar pronto, teste de fumaça, rodar os 130 exercícios, corrigir com o grader, parar o servidor.
- Se um harness falhar em mais de 5 dos 10 primeiros exercícios por erro de infraestrutura, pare esse harness, marque UNSUPPORTED com evidência e siga para o próximo.

### 5.0b Julgamento final (feito pela LLM operadora)
A nota de cada exercício é decidida por você, a LLM que conduz os testes, em duas camadas. O modelo avaliado nunca participa.

**Camada 1: grader oficial (objetiva).** `grader/run.py` na cópia final, como descrito na seção 5.0. Aprovado no grader é condição necessária.

**Camada 2: revisão da LLM operadora (assertividade).** Para cada exercício aprovado pelo grader, e para uma amostra de 20% dos reprovados, leia o `git diff` final contra o commit "base" e o `CHALLENGE.md`. Decida se a entrega é legítima. Reprove, com o motivo registrado, se encontrar:
- testes públicos, scripts (`scripts/*.sh`), `global.json`, `package.json` de teste ou configuração de lint alterados, apagados ou desativados para passar;
- respostas fixas feitas para os casos de teste em vez da regra pedida;
- API pública quebrada ou renomeada, contra o que o enunciado pede;
- mudanças fora do escopo que introduzem risco: arquivos não relacionados reescritos, dependências novas sem motivo;
- trechos que o próprio enunciado proíbe: segurança desligada, validação removida etc.

A revisão é **cega**: um script do operador renomeia as entregas para códigos aleatórios (por exemplo `E017-H3`) e remove do diff tudo que identifica o harness, como `.fh/`, arquivos de sessão e comentários de assinatura. Só depois do julgamento de todos os harnesses o código é desfeito. Guarde a tabela de correspondência.

**Resultado por exercício:** `aprovado` = grader aprovou **e** a revisão confirmou. Reporte três números por harness: aprovação no grader, aprovação final, e quantas aprovações do grader a revisão derrubou, com os motivos agrupados. O ranking principal usa a aprovação final. O ranking só pelo grader aparece ao lado, para comparar com as rodadas anteriores.

**Nota de qualidade (0–5)** para toda entrega aprovada: clareza, aderência ao enunciado, testes adicionados, simplicidade. Rubrica fixa, escrita antes de ver a primeira entrega e guardada em `relatorio/rubrica.md`. Não entra no critério de aprovação, só no relatório.

**Consistência do juiz.** Reavalie às cegas 10% das entregas, sorteadas com semente fixa, numa segunda passada separada da primeira, e reporte a concordância. Se ela ficar abaixo de 90%, revise a rubrica, reavalie tudo e registre.

**Só o resultado final conta (decisão 1).** O que o harness diz sobre si, as tentativas intermediárias e quantas rodadas ele precisou não entram no julgamento nem no relatório como acerto ou erro. Quem errou duas vezes e acertou na terceira está certo. Tempo e tokens gastos continuam sendo medidos como custo (seção 6).

### 5.0a Paralelização entre exercícios: rampa gradual até 10
O objetivo é terminar o conjunto mais rápido, com vários exercícios ao mesmo tempo, sem estragar nem o desempenho nem a comparação. Nesta rodada o teto é 10; o uso extremo fica para a fase 8.

**1. Calibração (uma vez, antes do primeiro harness, fora da comparação).**
- Servidor recém-iniciado, com o `fh` no modo 1 agente e os primeiros 60 exercícios do catálogo. Essas execuções não contam, e a memória do `fh` usada aqui é descartada.
- Degraus de concorrência de exercícios: 1 (linha de base), 2, 3, 4, 5, 6, 7, 8, 9, 10. Cada degrau dura pelo menos 8 minutos ou 5 exercícios concluídos, o que vier por último. Um exercício novo só entra quando outro termina, para manter N constante.
- No fim de cada degrau, imprima e grave em `WORKDIR/calibracao.csv`:
  - tok/s de saída por fluxo: mediana e P10;
  - tok/s agregado: saída total ÷ tempo do degrau;
  - requisições rodando e em fila, média e pico;
  - uso de KV cache, média e pico, e preempções;
  - TTFT P50 e P90;
  - aceitação do MTP;
  - uso de CPU e RAM do host (builds de .NET e Node em paralelo competem por CPU);
  - potência média da GPU;
  - erros e timeouts.
- **De 1 a 5:** sobe um degrau de cada vez. Só para antes de 5 se aparecer erro 5xx, falta de memória, preempção ou KV cache acima de 95%.
- **De 5 a 10** ("chega a 10 sem prejudicar o desempenho?"): só suba para N+1 se, no degrau N, valerem **todas** estas condições em relação ao degrau 1:
  - o tok/s mediano por fluxo continua ≥ 70% do valor do degrau 1;
  - o TTFT P90 não passou de 2× o do degrau 1 nem de 15 s;
  - o KV cache ficou abaixo de 90%, sem preempções;
  - a CPU do host ficou abaixo de 90% em média (senão o gargalo são os builds, e mais paralelismo não ajuda);
  - nenhum erro 5xx, falta de memória ou timeout de exercício a mais do que no degrau 1.

  O primeiro degrau que violar uma condição é descartado, e N_MAX = o último degrau que cumpriu todas. Se todos cumprirem, N_MAX = 10.
- Registre a decisão com os números: tabela e gráfico de tok/s por fluxo e agregado contra N, marcando o degrau que parou a rampa e o motivo.

**2. Execução principal (todos os harnesses, mesma escala).**
- A concorrência depende da posição do exercício no catálogo, não do harness. Ela sobe como na calibração, sem pular degraus:
  - exercícios 1–4: 2 em paralelo;
  - 5–8: 3;
  - 9–12: 4;
  - 13–16: 5;
  - e daí +1 a cada 4 exercícios até N_MAX, onde fica até o exercício 130.

  Assim cada exercício roda, em todos os harnesses, com a mesma concorrência, e a comparação exercício a exercício continua justa.
- A cada degrau e depois a cada 10 exercícios concluídos, imprima a mesma linha de números da calibração, com o harness e o N atual, e grave-a em `WORKDIR/runs/<harness>/tps.csv`.
- **Freio de segurança.** Se, durante um harness, aparecer erro 5xx, falta de memória ou KV cache acima de 95% por mais de 2 minutos, pare de lançar exercícios novos até a fila esvaziar e continue no mesmo N. Nunca mude o servidor, e registre cada acionamento com o horário; isso é resultado do harness. Se o freio acionar três vezes no mesmo harness, avise no relatório que N_MAX foi alto demais para aquele harness.
- O timeout continua 900 s de parede em todos os degraus. Reporte a taxa de timeout por degrau.
- O modo máximo do `fh` soma workers aos exercícios em paralelo: com N_MAX exercícios pode haver bem mais requisições simultâneas. Não reduza o N desse modo; a disputa pela GPU é o custo do fan-out e aparece como fila no relatório.

**Ordem de execução (decisão 7): o `fh` sempre primeiro.** 1) `fh` modo 1 agente; 2) `fh` modo máximo; 3) OpenCode; 4) Qwen Code; 5) Claude Code; 6) DeepSeek Harness. Cada um com servidor recém-iniciado.

### 5.1 Frankenstein Harness, modo 1 agente (primeiro)
Configuração por modo num diretório só de leitura, `WORKDIR/fh-config/<modo>/config.json`, passado com `FH_CONFIG_HOME`:

```json
{
  "endpoint": "http://<proxy>:8001/v1",
  "model": "frankenstein-v2",
  "contextWindow": 131072,
  "maxConcurrency": 1,
  "telemetry": false,
  "sampling": {
    "thinking": {"temperature": 1.0, "topP": 0.95, "topK": 20},
    "instant":  {"temperature": 1.0, "topP": 0.95, "topK": 20}
  }
}
```

Os campos:
- `contextWindow` deve ser igual ao `max-model-len` usado.
- `maxConcurrency: 1` desliga os workers: o planejador não divide a tarefa, e é exatamente o que o `fh eval --runner fh-single` faz.
- Deixe `maxTaskTokens` no padrão (800 000) nos dois modos e registre. O `fh` para ao atingir esse limite.

Comando por exercício, dentro do contêiner, com `/work` como diretório:

```bash
FH_CONFIG_HOME=/fh-config \
FH_HOME=/fh-state \
FH_API_KEY=<chave do exercício> \
FH_METRICS_URL=http://<proxy>:8001/metrics \
fh run --auto --yes --mode yolo --json --cwd /work "<prompt de tarefa>"
```

Pontos obrigatórios:
- **Memória persistente entre exercícios (decisão 2).**
  - `/fh-state` é montado de `WORKDIR/fh-state/<modo>/`, com leitura e escrita, compartilhado por todos os exercícios daquele modo e fora de `/work`, para não aparecer no diff nem na correção.
  - Começa vazio no início de cada modo; o modo 1 agente não passa memória para o modo máximo.
  - Vários `fh` em paralelo escrevem no mesmo banco de skills. O `fh` espera até 30 s pelo lock do SQLite (testado com 8 escritores simultâneos), então isso é suportado.
  - Com exercícios em paralelo, a ordem exata do aprendizado não é determinística; registre isso.
  - Ao terminar cada modo, guarde uma cópia de `WORKDIR/fh-state/<modo>/` e a saída de `fh activity` e `fh stats` (com `FH_HOME` apontando para ela).
  - Para os outros harnesses: se algum tiver memória persistente nativa (por exemplo, arquivos de memória por usuário), dê a ele o mesmo tratamento: um diretório de estado persistente por harness, vazio no início. Registre o que cada um tem.
- **Sem `--keep` (decisão 1).** O que o `fh` deixar no disco ao terminar é a resposta dele: se a verificação interna reprovou e ele corrigiu, ótimo; se reprovou até o fim e ele desfez a mudança, a entrega é o estado original. Não corrija patches rejeitados nem estados intermediários: só a entrega final importa. Antes da correção e da revisão às cegas, apague `.fh/` da cópia, porque lá ficam os patches rejeitados.
- **Sem `--sandbox`** na comparação principal: o contêiner já isola todos os harnesses igualmente.
- **Verificação com os scripts do exercício (decisão 3, já corrigido no `fh`).** Quando o repositório tem `scripts/test.sh`, o `fh` verifica com os scripts que o próprio repositório declara (`scripts/build.sh`, `typecheck.sh`, `lint.sh`, `test.sh`, na ordem, com `bash`), em vez de inferir `dotnet test` ou `npm test`. No jg-eng-tests isso dá `bash scripts/lint.sh` e `bash scripts/test.sh`. Confirme nos pilotos (fase 0, item 6). Se ainda aparecer comando inferido, pare e corrija antes da execução que conta.
- Guarde o JSON de `--json` como dado bruto (não entra na nota): rodadas de verificação, checks executados, workers, tokens, tool calls, chamadas reparadas ou malformadas, skills usadas (`skillsUsed`), aceitação do MTP.

### 5.2 Frankenstein Harness, modo máximo de agentes (segundo)
Mesmo comando, com `maxConcurrency: 16` (o teto de workers por exercício, independente do `max-num-seqs`). Isso não força 16 agentes. É o teto: o planejador divide a tarefa em até 16 subtarefas com arquivos disjuntos, só quando elas são independentes. O regulador começa com 2 simultâneos e sobe conforme o `/metrics`. Também podem surgir workers "extra" para arquivos que ninguém possuía e um corretor por diretório em rodadas de reparo. Todos ficam restritos à pasta, pelo contêiner.

Registre por exercício:
- subtarefas planejadas;
- workers criados, incluindo os extra;
- tokens por worker;
- tempo em fila no servidor (`num_requests_waiting` e o histograma de fila no intervalo do exercício).

Com `max-num-seqs` fixo, esse modo pode saturar; isso é resultado, não erro. Registre também em quantos exercícios o planejador não dividiu nada, porque nesses exercícios os dois modos são equivalentes.

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
Localize o repositório oficial e verifique se aceita endpoint OpenAI-compatível customizado e tool calling com este modelo. Se não aceitar, marque UNSUPPORTED com a evidência (trecho de código ou documentação, versão) e siga. Se aceitar, use modo não interativo, com as mesmas permissões e sem web.

## 6. Fase 5: o que medir (por exercício e agregado por harness)
- **Qualidade:**
  - aprovado, nota (score/maximum), critérios aprovados por nível (mínimo, esperado, excelente);
  - erros de infraestrutura (fora do denominador, listados), timeouts;
  - aprovação no grader, aprovação final (grader + revisão da LLM operadora) e aprovações derrubadas pela revisão, com os motivos;
  - nota de qualidade 0–5 da LLM operadora;
- **Tempo:**
  - tempo de parede total do harness;
  - por exercício: média, mediana, P90;
  - tempo até o primeiro token: mediana e P90 por requisição.
- **Tokens:**
  - entrada, cache, entrada sem cache, saída e raciocínio (estimado);
  - total por exercício e por harness;
  - tokens por exercício aprovado, total e sem cache.
- **Velocidade por degrau de concorrência (1 a N_MAX):**
  - tok/s por fluxo e agregado, fila, KV cache, TTFT, aceitação do MTP e potência, em cada degrau e por harness;
  - a curva de tok/s agregado contra N é o gráfico principal desta parte.
- **Velocidade:**
  - tok/s por fluxo (saída ÷ (duração − TTFT)) e agregado;
  - concorrência média e de pico (do log do proxy);
  - fator de paralelização: soma das durações das requisições ÷ tempo de parede do exercício, descontando o tempo em fila;
  - aceitação do MTP, geral e por posição.
- **Comportamento:**
  - turnos, requisições, chamadas de ferramenta, tool calls malformadas;
  - execuções de `test.sh`/`lint.sh`, contadas nos logs do harness ou nos comandos registrados;
  - exercícios que terminaram sem declarar conclusão ou sem nenhuma mudança;
  - `fh`: agentes, skills aprendidas e skills usadas por exercício (só para entender custo e aprendizado; não entram na nota);
  - agentes por exercício (Frankenstein Harness);
  - tentativas de rede bloqueadas.
- **Hardware:**
  - uso médio e de pico de GPU, VRAM, potência média, temperatura;
  - CPU e RAM do host por degrau, para separar gargalo de GPU de gargalo de build;
  - energia por harness (integral da potência no tempo) e energia por exercício aprovado.

## 7. Fase 6: análise e relatório

### Análise
- **Ranking.** Taxa de aprovação final (grader + revisão da LLM operadora) com intervalo de Wilson de 95%. Ao lado, o ranking só pelo grader.
- **Comparação pareada.** Exercício a exercício, entre cada par de harnesses: teste binomial exato bicaudal nos pares discordantes (McNemar exato) e intervalo de 95% da diferença de taxas por bootstrap pareado (10 000 reamostragens, semente fixa registrada). Corrija as comparações múltiplas por Holm. Não diga "melhor" sem apoio estatístico; use "sem diferença conclusiva" quando for o caso. Com 130 exercícios, diferenças de poucos pontos quase nunca são conclusivas; diga isso.
- **Recortes.** Resultados por trilha, stack (.NET, React, Angular) e nível.
- **Por tipo.** Exercícios cujo código inicial devolve `<unimplemented>` (contrato de saída implícito) contra os que já têm código real.
- **Escala.** O que a paralelização entre exercícios rendeu:
  - tempo total de parede real contra o de rodar um exercício por vez (soma das durações);
  - tok/s agregado e por fluxo por degrau;
  - onde a curva achata e o motivo: KV cache, fila ou queda da aceitação do MTP.
  - Diga também se a aprovação mudou entre degraus. Como os degraus seguem a ordem do catálogo, qualquer diferença mistura dificuldade com concorrência. Aponte a confusão e não conclua causalidade.
- **Aprendizado do `fh` (decisão 2).**
  - Taxa de aprovação na primeira metade contra a segunda metade do catálogo, e nos exercícios em que alguma skill aprendida foi usada contra os demais.
  - Quantas skills foram aprendidas, promovidas e postas em quarentena.
  - Não conclua causalidade: a ordem mistura dificuldade com aprendizado. Se houver tempo, uma variante com memória desligada (FH_HOME vazio por exercício) no mesmo modo isola o efeito; marque-a como variante.
- **1 agente contra máximo.** Aplique a regra de fan-out do projeto (`docs/FANOUT.md`), que vale para exercícios em que houve divisão. O modo máximo compensa se não perder qualidade e se: (a) a mediana de tempo ficar ≤ 80% da do modo 1 agente, ou a aprovação subir pelo menos 5 pontos; e (b) os tokens sem cache por exercício aprovado ficarem ≤ 2× os do modo 1 agente. Mostre também o resultado para todos os 130.
- **Com repetições.** Se houver repetições, reporte a média por exercício e a variância entre elas. As estatísticas pareadas usam a taxa média por exercício.
- **Referência externa**, marcada como ambiente diferente e fora das estatísticas: a rodada anterior do Frankenstein V2 com harness caseiro (57,7%, temperatura 0,2) e os resultados do Claude Opus 5.5 (50,0%) e do Sonnet 5.5 (47,7%) no Claude Code.

### Avaliação independente das instruções e dos testes
Feita pela LLM operadora, nunca pelo Qwen. Só o operador lê os graders, e só depois das execuções. Por exercício, avalie:
- a clareza do enunciado;
- se o formato de saída está especificado ou só implícito;
- se o teste público verifica o que o grader privado exige (liste os critérios privados sem equivalente público);
- ambiguidades que forçam adivinhação;
- testes instáveis: rode o grader 3 vezes na referência e nos 5 exercícios com mais discordância entre harnesses;
- se a solução de referência passa no próprio grader (fase 0).

Use também o sinal empírico: critérios que nenhum harness passou e critérios que falham só por formato. Termine com uma classificação (claro, ambíguo, defeituoso) e sugestões de correção por exercício.

### Entregáveis em `WORKDIR/relatorio/`
1. `relatorio.md` e `relatorio.txt` com toda a análise e todos os números. Primeira seção: resumo de uma página com o ranking, a comparação 1 agente × máximo, N_MAX, os resultados dos benchmarks públicos (fase 7), a capacidade máxima medida na fase 8 e a lista de NOT_RUN/UNSUPPORTED.
2. `relatorio.pdf` com tabelas e gráficos: ranking com intervalos, tokens, tempo e velocidade por harness, aprovação por trilha/stack/nível, tok/s por fluxo e agregado contra o número de exercícios em paralelo (calibração e cada harness), e 1 agente × máximo.
3. `results.csv` e `results.json` por exercício e harness, os logs do proxy, do `/metrics` e do `nvidia-smi`, e os JSONs do grader.
4. `MANIFEST.json`:
   - commits (jg-eng-tests e `fh`);
   - versões (vLLM, CUDA, driver, cada harness, LiteLLM se usado, .NET, Node, pwsh, language servers, digest da imagem);
   - o comando exato do servidor e as flags de fallback usadas;
   - amostragem imposta, `max-model-len`, `max-num-seqs` final, N_MAX e a escala de rampa usada;
   - as variantes de servidor da fase 8, cada uma com o comando exato;
   - para cada benchmark público: versão/commit do conjunto de tarefas, versão do Harbor/Pier, lista exata de tarefas rodadas e o motivo de cada tarefa não rodada;
   - método de identificação por harness, regra de firewall;
   - horários de início e fim de cada fase e de cada harness.

## 8. Fase 7: benchmarks públicos (todos os harnesses, `fh` primeiro)
Objetivo: rodar cinco benchmarks públicos de código com todos os harnesses, como no jg-eng-tests, para comparar com o resto do mercado. Valem as mesmas regras: decisões 1, 2, 6 e 7, proxy de medição, amostragem imposta, servidor recém-iniciado por harness, rede bloqueada fora do endpoint. Nada desta fase roda antes de as fases 0 a 6 terminarem.

| Benchmark | Tarefas | Runner | Onde estão as tarefas | Observação |
|---|---|---|---|---|
| Terminal-Bench 4.0 | 66 | Harbor | Harbor Hub (`harbor datasets list`; nome provável `terminal-bench@4.0`) | timeout fixo de 8 h por tarefa |
| DeepSWE v1.1 | 113 | Pier (fork do Harbor da Datacurve) | `github.com/datacurve-ai/deep-swe` (`tasks/`) | sem internet; nota pelo que for **commitado** (`git diff base..HEAD`) |
| CWE-bench v1 | 120 (privadas) | Harbor | conjunto avaliado é privado (Collinear e Artificial Analysis); ver `cwe-bench.com` | juízes são modelos de fronteira, nunca o Qwen |
| Vibe Code Bench | 50 públicas (validação) | scaffold próprio (OpenHands) + avaliador com agente de navegador | `github.com/vals-ai/VibeCodeBench-Openhands-Scaffold` | o `fh` precisa de um adaptador próprio (8.5) |
| FrontierSWE v2 | 34 | px-eval sobre o Harbor | `github.com/Proximal-Labs/frontier-swe-v2` | até 20 h por tarefa, várias exigem GPU |

### 8.0 Mecânica comum
- **Ordem (decisão 7).** Em cada benchmark: `fh` modo 1 agente, `fh` modo máximo, OpenCode, Qwen Code, Claude Code, DeepSeek Harness. Entre benchmarks: Terminal-Bench 4.0, DeepSWE v1.1, CWE-bench v1, Vibe Code Bench, FrontierSWE v2 (o mais longo por último).
- **Mesmo conjunto de tarefas para todos.** Rode o benchmark inteiro. Se o tempo não der para todos os harnesses, escolha um subconjunto fixo com semente registrada (`--n-tasks N --sample-seed 0` no Pier, `-i/-x` ou `-l` no Harbor), decidido **antes** do primeiro harness. Todos os harnesses rodam exatamente esse subconjunto.
- **Paralelismo.** Tarefas em paralelo com `-n` do Harbor/Pier. A rampa é a mesma do jg-eng-tests: começa em 2, sobe até 5 e só passa disso até N_MAX (fase 4) se os critérios da seção 5.0a continuarem valendo. Tarefas longas consomem CPU e memória do host por horas: respeite `cpus`/`memory_mb` de cada `task.toml` e reduza `-n` antes de reduzir recursos de tarefa. Registre o `-n` usado.
- **Timeouts.** Use os timeouts oficiais de cada benchmark, sem multiplicadores. Se for preciso encurtar, isso vira variante com nome próprio, fora da comparação.
- **Piloto.** Antes de cada harness em cada benchmark, rode 2 tarefas fora da contagem para validar a ligação: o agente instalou, falou com o proxy usando a chave certa, o verificador rodou e gravou a nota.
- **Nota.** Vale a nota do verificador oficial de cada benchmark, sobre a entrega final (decisão 1). Para o CWE-bench e o Vibe Code Bench, os juízes oficiais são modelos externos: registre quais. Eles nunca são o Qwen nem um harness avaliado (decisão 6). Faça também uma revisão às cegas, pela LLM operadora, de 10% das tarefas aprovadas de cada harness, procurando trapaça (teste alterado, resposta fixa). Uma aprovação derrubada é reportada à parte; ela não muda a nota oficial.
- **Memória do `fh` (decisão 2).** Um diretório de skills por benchmark e por modo, vazio no início, passado com `--ak state_dir=...`. O adaptador copia a memória para cada tarefa e a funde de volta ao final, com trava, mesmo com tarefas em paralelo.
- **Medição.** Tudo passa pelo proxy (seção 3): tokens, TTFT, tok/s, aceitação do MTP, GPU. Some a isso o resultado do Harbor/Pier (`result.json` de cada job).

### 8.1 Como o `fh` entra nos benchmarks (adaptadores do repositório)
O repositório já traz os adaptadores em `integrations/harbor/fh_harbor/`, documentados em `integrations/harbor/README.md`:
- `fh_harbor.agent:FrankensteinHarness` para o Harbor (Terminal-Bench, CWE-bench, FrontierSWE);
- `fh_harbor.pier_agent:FrankensteinHarness` para o Pier (DeepSWE).

Eles instalam um binário Linux estático do `fh` dentro do contêiner de cada tarefa, rodam `fh run --auto --yes --mode yolo --json` com a instrução da tarefa e devolvem tokens e veredito ao Harbor/Pier.

O que foi verificado antes de entregar este plano (com o modelo simulado do `fh`, em Docker):
- **Harbor:** uma tarefa real teve nota 1,0 do verificador oficial. Três tarefas em paralelo tiveram 1,0 cada e a memória compartilhada fundiu as três.
- **Pier:** a instalação durante o build funcionou e o `fh` rodou sem internet. A chamada ao modelo foi barrada porque o proxy do Pier só deixa sair pelas portas 80 e 443 (veja 8.3). O caminho completo no Pier **não** foi confirmado; confirme no piloto.

Preparação, uma vez:
1. Compile o binário estático: `rustup target add x86_64-unknown-linux-musl`, instale `musl-tools`, e rode `CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release --target x86_64-unknown-linux-musl`. Confira com `file` que o binário é estático.
2. Instale Harbor e Pier em versões fixas, cada um no seu ambiente Python 3.12 (`uv tool install harbor`, `uv tool install datacurve-pier`), e registre as versões. Exporte `PYTHONPATH=<repo>/integrations/harbor`.
3. **Endpoint visível dos contêineres.** Os agentes rodam dentro de contêineres, então o proxy de medição precisa escutar num endereço que eles alcancem: o gateway do Docker (`docker network inspect bridge`, em geral `172.17.0.1`). Use `FH_ENDPOINT=http://host.docker.internal:8001/v1` com um overlay de compose (`extra_hosts: ["host.docker.internal:host-gateway"]`, passado com `--extra-docker-compose`) e `--allow-agent-host host.docker.internal`. Mantenha o firewall da seção 3: só o proxy é alcançável.
4. Os outros harnesses usam os agentes nativos do Harbor (`--agent opencode`, `--agent qwen-code`, `--agent claude-code`; confira com `harbor run --help` e `harbor agent schema <nome>`), configurados para o mesmo proxy e o mesmo `frankenstein-v2`, como nas seções 5.3 a 5.5. O DeepSeek Harness não tem agente nativo: se não houver como plugá-lo (adaptador próprio aceitando endpoint OpenAI-compatível), marque UNSUPPORTED com a evidência.

### 8.2 Terminal-Bench 4.0
```bash
harbor run -d terminal-bench@4.0 \
  --agent fh_harbor.agent:FrankensteinHarness -m openai/frankenstein-v2 \
  --ak binary=<fh estático> --ak max_concurrency=1 --ak state_dir=WORKDIR/fh-state/tb4-1agente \
  --ae FH_ENDPOINT=http://host.docker.internal:8001/v1 --ae FH_API_KEY=<chave do lote> \
  --extra-docker-compose host-gateway.yaml --allow-agent-host host.docker.internal \
  -n <degrau da rampa> -o WORKDIR/bench/tb4/fh-1agente
```
- Modo máximo: o mesmo comando com `--ak max_concurrency=16` e um `state_dir` próprio.
- Depois rodam OpenCode, Qwen Code, Claude Code e DeepSeek, com o mesmo `-d`, o mesmo `-n` e as mesmas tarefas.
- Confira o nome exato do dataset com `harbor datasets list` antes de começar.
- Referência externa (ambiente diferente, fora das estatísticas): Claude Opus 5.5 65,15%, Sonnet 5.5 64,14%, GPT-6 Astra 59,60%.

### 8.3 DeepSWE v1.1 (Pier)
- Clone `datacurve-ai/deep-swe`, confirme que as tarefas são da v1.1 (README, `task.toml`, imagem `...-v1.1`) e registre o commit.
- **Binário por URL.** O Pier embute o agente na imagem durante o build, então o binário precisa vir por URL. Sirva-o de uma pasta do host: `python3 -m http.server 8090 --bind 172.17.0.1`, e passe `FH_BINARY_URL=http://172.17.0.1:8090/fh` mais `FH_BINARY_SHA256`.
- **Endpoint na porta 80 ou 443.** Na execução, a única saída é o proxy Squid do Pier, que só aceita essas portas. Ponha uma segunda escuta do proxy de medição em `172.17.0.1:80` e use `FH_ENDPOINT=http://172.17.0.1/v1`. Confirme no piloto que o `fh` recebe resposta.
- **Commit obrigatório.** A nota sai de `git diff base..HEAD`, então use `--ak commit=true`: o `fh` commita quando a verificação passa ou quando não havia como verificar.

```bash
pier run -p deep-swe/tasks \
  --agent-import-path fh_harbor.pier_agent:FrankensteinHarness -m openai/frankenstein-v2 \
  --ae FH_ENDPOINT=http://172.17.0.1/v1 --ae FH_BINARY_URL=http://172.17.0.1:8090/fh \
  --ak commit=true --ak max_concurrency=1 --ak state_dir=WORKDIR/fh-state/deepswe-1agente \
  -n <degrau> -o WORKDIR/bench/deepswe/fh-1agente
```
- **Outros harnesses.** O Pier tem OpenCode e Claude Code nativos. Para o Qwen Code e o DeepSeek, que não estão no Pier, tente as mesmas tarefas no Harbor. Se ele não suportar o formato 1.3 dessas tarefas (`verifier.collect`, `environment_mode = "separate"`), marque UNSUPPORTED com a evidência.
- A Epoch apontou problemas em pelo menos 23 das 113 tarefas. Se houver lista pública delas, reporte também a nota sem essas tarefas.
- Referência externa: o líder público tinha 75,4%, e a faixa dos nove modelos da v1.1 ia de 12% a 70%.

### 8.4 CWE-bench v1
- Antes de tudo, confirme o acesso. O conjunto avaliado (120 tarefas, 73 CWEs, 8 linguagens) é privado. Veja em `cwe-bench.com` se há tarefas públicas ou acesso por pedido. Sem acesso, marque NOT_RUN com o motivo e **pergunte ao dono do projeto** antes de substituir por outro conjunto.
- Com acesso: Harbor com o mesmo adaptador (seção 8.2).
- A nota vem do painel oficial de juízes (três modelos de famílias diferentes), que precisa das chaves de API desses provedores, fornecidas pelo dono do projeto. Registre modelos, versões e custo. Os juízes nunca são o Qwen.
- As tarefas rodam sem internet: confirme que só o endpoint do proxy é alcançável.

### 8.5 Vibe Code Bench (50 especificações públicas)
- Clone `vals-ai/VibeCodeBench-Openhands-Scaffold` e leia como cada aplicação é gerada e empacotada: código, Docker Compose e estado do banco. Leia também como o avaliador com agente de navegador é chamado.
- **Adaptador do `fh` (escreva em `WORKDIR/tools/vcb_fh/`).** Para cada especificação, ele cria o mesmo espaço de trabalho vazio que o scaffold cria, roda `fh run --auto --yes --mode yolo --json` com a especificação como tarefa, e empacota o resultado exatamente no formato que o avaliador espera. Valide com 2 especificações antes de rodar as 50.
- **Outros harnesses.** Use o mesmo adaptador, trocando o comando do agente, para que todos sejam empacotados e avaliados igual. O OpenHands do scaffold não é um dos harnesses comparados; não o use.
- **Avaliador.** Ele dirige um navegador com um modelo. Esse modelo não pode ser o Qwen (decisão 6): use o modelo oficial do avaliador, com chave fornecida pelo dono do projeto, e registre-o. Se não for possível, marque NOT_RUN com o motivo.
- Se o adaptador não ficar confiável no piloto, marque NOT_RUN para todos os harnesses, nunca só para alguns.

### 8.6 FrontierSWE v2 (por último)
- Clone `Proximal-Labs/frontier-swe-v2` e use o runner indicado no README (px-eval, sobre o Harbor). Registre o commit.
- **GPU.** Muitas tarefas pedem GPU (`gpus` no `task.toml`), e a GPU da máquina já está ocupada pelo vLLM. Rode só as tarefas com `gpus = 0`. As que pedem GPU ficam NOT_RUN ("ambiente: GPU ocupada pelo modelo avaliado"), a menos que exista uma segunda GPU livre; nesse caso, registre qual.
- **Tempo.** Cada tarefa pode durar até 20 h; o oficial é mean@5. Rode 1 tentativa por tarefa e reporte como mean@1, deixando claro que não é comparável ao mean@5 do placar. Com tempo sobrando, faça mais tentativas só nas tarefas do `fh` e do melhor outro harness.
- Referência externa: GPT-6 Astra 65,5%, Claude Opus 5.5 62,3%, Sonnet 5.5 61,9% (mean@5, com o harness próprio da Proximal).

### 8.7 Relatório dos benchmarks
Para cada benchmark e harness, reporte:
- nota oficial com intervalo de Wilson de 95% (ou média com intervalo por bootstrap, quando a nota for contínua);
- comparação pareada tarefa a tarefa contra o `fh` modo 1 agente e o `fh` modo máximo;
- tokens (total e sem cache) por tarefa resolvida, tempo por tarefa, tok/s, `-n` usado;
- tarefas NOT_RUN e UNSUPPORTED com motivo;
- a referência externa do placar, marcada como ambiente diferente (outro modelo e, às vezes, outro harness).

Inclua tudo em `relatorio.md`, no PDF e em `results.csv` (coluna `benchmark`).

## 9. Fase 8: teste de capacidade do `fh` em uso extremo (por último, depois de tudo)
Objetivo (decisão 5): quantos usuários simultâneos o servidor aguenta com o `fh`, e com que tok/s por usuário. Isso fica fora da comparação entre harnesses, então aqui **é permitido** testar variantes de servidor, cada uma com nome próprio e servidor reiniciado.

**Definições.**
- **Usuário** = uma sessão do `fh` resolvendo um exercício. Cada usuário recebe uma cópia nova de um exercício, percorrendo o catálogo em ciclo. Use o modo 1 agente como carga principal, e depois repita os degraus principais no modo máximo.
- **tok/s de uma atividade** = tokens de saída de uma requisição ÷ (duração − TTFT), do log do proxy.
- **tok/s por usuário:**
  - durante a geração: média do tok/s das requisições daquele usuário, ponderada pelos tokens;
  - efetivo: saída total da sessão ÷ tempo de parede da sessão, que inclui o tempo de ferramentas e de build.

  Reporte os dois: o primeiro mede o servidor, o segundo o que o usuário sente.
- **tok/s médio por usuário no degrau** = média e mediana entre os usuários do degrau, com P10 e P90.

**Técnica 1: carga real com agentes (principal).**
- Servidor de referência (a mesma configuração da comparação) recém-iniciado, com a memória do `fh` vazia.
- **Degraus:** começa com **20 usuários ao mesmo tempo**, todos lançados juntos. Depois: 24, 28, 32, 40, 48, 56, 64, 80, 96, 128, e segue dobrando enquanto não parar.
- Cada degrau dura pelo menos 15 minutos em carga constante: quando um usuário termina, outro entra na hora.
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

  Se 20 já não for confortável, desça em degraus de 4 (16, 12, 8) até achar o confortável.

**Técnica 2: repetição das sessões gravadas (isola o servidor).**
- Com os logs do proxy da comparação principal (requisições completas do `fh`, com os intervalos reais entre elas), escreva um reprodutor que dispara N sessões gravadas ao mesmo tempo. Os degraus são os mesmos da técnica 1 e continuam além do limite dela, até 256 ou até quebrar.
- Mande os mesmos prompts com os tempos originais entre chamadas, sem executar ferramentas nem builds. Isso tira a CPU do caminho e mostra o limite só do servidor.
- A diferença entre o limite das técnicas 1 e 2 mostra quanto do limite vem do host (builds) e quanto vem da GPU.

**Técnica 3: carga sintética padronizada.**
- Use `vllm bench serve` da versão instalada (confira as opções em `--help`), com distribuições de tamanho de entrada e saída iguais às medidas nos logs, prefixo compartilhado igual ao observado, e `--max-concurrency` em 1, 8, 16, 32, 64, 128 e 256.
- Faça também uma varredura por taxa de chegada (`--request-rate`) até saturar, para encontrar a vazão máxima sustentável em req/s e tok/s.
- Esses números são comparáveis com outros servidores e outras GPUs.

**Técnica 4: pico súbito.** De 0 para o limite "aceitável" de uma vez. Meça quanto tempo leva até o TTFT estabilizar e se há erros ou preempções no pico.

**Técnica 5: resistência.** 60 minutos contínuos no nível "confortável". Procure degradação ao longo do tempo: tok/s caindo, VRAM ou RAM subindo, temperatura, erros tardios.

**Variantes de servidor (cada uma reiniciada e medida com as técnicas 2 e 3, e com a técnica 1 nos degraus perto do limite):**
- `max-num-seqs` 128 e 256;
- `gpu-memory-utilization` 0.95;
- MTP com `num_speculative_tokens` 1 e sem MTP (sob lote grande, a especulação pode render menos que o custo);
- `max-model-len` 65536 (mais sequências cabem no KV cache);
- `max-num-batched-tokens` maior ou menor e o chunked prefill da versão instalada.

Confira cada flag em `vllm serve --help`. Registre para cada variante a capacidade confortável e o limite, e diga qual configuração maximiza usuários confortáveis e qual maximiza a vazão agregada. Podem não ser a mesma.

**Entregável da fase 8:** `WORKDIR/relatorio/capacidade.md` e seção própria no PDF, com:
- gráficos de tok/s por usuário (mediana e P10) e tok/s agregado contra usuários simultâneos, para cada técnica e variante;
- a tabela de capacidade em três níveis;
- a recomendação final: quantos usuários simultâneos do `fh` a GPU aguenta com conforto, com qual configuração e com que tok/s médio por usuário.

Ao atingir qualquer limite (tempo, disco, fase em EXECUTAR_ATE), gere o relatório com o que foi medido e liste o que ficou NOT_RUN ou UNSUPPORTED, com o motivo.

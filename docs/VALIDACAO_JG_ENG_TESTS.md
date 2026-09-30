# Prompt de execução: validação de harnesses com o Frankenstein V2 (jg-eng-tests)

Cole este documento inteiro numa sessão do Claude Code com terminal na máquina da GPU. Execute, não apenas descreva. Não invente resultados: o que não for medido fica NOT_RUN ou N/D, com o motivo.

Nomes usados aqui: **Frankenstein V2** é o modelo (Qwen3.8-27B NVFP4 + MTP=3 no vLLM). **Frankenstein Harness** (`fh`) é o nosso harness. Não confunda os dois no relatório.

## CONFIGURAÇÃO (preencher antes de colar)
- REPO: https://github.com/dfnb/jg-eng-tests, fixado no commit `36cbe4741e5730fae876fae3bff6fa167fb18cb7`. Se o HEAD for outro, pare e avise.
- GPU: `<ex.: RTX PRO 6000 Blackwell 96 GB>`
- WORKDIR: `<ex.: /workspace/harness-eval>` (precisa de ~150 GB livres: 6 execuções × 130 cópias com node_modules e bin/obj)
- FRANKENSTEIN_HARNESS: repositório https://github.com/cardimvitor/frankenstein-harness, branch `ccr-3bc51f62-prkdq0`. Compilar com `cargo build --release` e usar `target/release/fh`. Registre o commit.
- EXERCICIOS_EM_PARALELO: rampa 4 → 8 → 12 → 16 → 20, com teto N_MAX = 20 (seção 5.0a). A escala de rampa é a mesma para todos os harnesses
- TIMEOUT_POR_EXERCICIO: 900 s de parede, contando do início do harness até o processo terminar
- REPETICOES: 1 (3 se houver tempo, só para os 2 melhores harnesses)
- AMOSTRAGEM: temperatura 1,0, top_p 0,95, top_k 20, igual para todos e imposta pelo proxy (seção 2)
- EXECUTAR_ATE: fase `<0 a 6>`

Estimativa de tempo, para planejar: com a rampa até 20 em paralelo, o pior caso (todos os exercícios estourando o timeout) cai de ~8 h para ~2 h por harness, ou ~12 h para as 6 execuções, mais a calibração. O normal fica bem abaixo disso. A sessão precisa conseguir retomar sem refazer o que já terminou (seção 4.0).

## 0. Regras que valem a sessão inteira
1. Os agentes avaliados só veem a pasta `exercise/` de cada desafio. Nunca `solution/`, `grader/`, `EVALUATION.md`, o `.git` do jg-eng-tests, outros exercícios ou resultados de outros harnesses. Você (operador) nunca resolve exercício, nunca dá dica e nunca cola conteúdo do grader em prompt.
2. Um modelo só: o Frankenstein V2 servido pelo nosso vLLM. Nenhum harness pode chamar modelo externo. Bloqueie a saída de rede dos agentes (só o proxy local é alcançável) e prove pelos logs do proxy e do firewall que 100% das chamadas de modelo foram para o vLLM local.
3. Mesmo prompt de tarefa para todos os harnesses, mesmo timeout, a mesma escala de concorrência de exercícios (a rampa da seção 5.0a, amarrada à posição do exercício no catálogo), mesma amostragem, mesma ordem de exercícios (a do `catalog.json`).
4. Cada harness roda num vLLM recém-iniciado (processo novo, cache vazio). O mesmo vale entre os dois modos do Frankenstein Harness.
5. Não mude a configuração do servidor entre harnesses. Se algo precisar mudar, rode como variante separada, com nome próprio, fora da comparação principal.
6. Não grave chaves ou tokens em arquivo, log ou relatório. Nos logs, identifique chaves pelo SHA-256 truncado.
7. Não altere nenhum arquivo do jg-eng-tests, nem os graders. Correções sugeridas vão só para o relatório.
8. Um erro ou resultado estranho do nosso harness (`fh`) é um achado, não algo a esconder. Registre com evidência. Não corrija o `fh` no meio da comparação: isso invalidaria a execução. Se a correção for indispensável, termine a execução, corrija, e rode de novo como variante com nome próprio.

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

## 2. Fase 1: servidor, configuração validada do Frankenstein V2
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
- `max-model-len`: 131072 se couber na GPU; mínimo 65536. Na rodada anterior, 32768 estourou num exercício, e Claude Code e OpenCode têm prompt de sistema grande. Registre o valor usado e use o mesmo valor em `FH_CONTEXT_WINDOW` (seção 5.5).
- Se o servidor travar ao iniciar com MTP no Blackwell (SM120), aplique a correção que resolveu antes: `--quantization modelopt_fp4 --block-size 128` com `VLLM_HAS_FLASHINFER_CUBIN=1`. Registre qual foi usado.
- Confira cada flag com `vllm serve --help` da versão instalada; não use sintaxe de memória. Se `--override-generation-config` não existir, use o equivalente da versão e registre. De qualquer forma, o proxy impõe a amostragem (seção 3), porque os harnesses mandam os próprios valores por requisição e o padrão do servidor não os sobrescreve.
- `max-num-seqs`: 16 foi validado na PRO 5000, mas com até 20 exercícios em paralelo (e o modo máximo do `fh` abrindo vários workers por exercício), 16 vira fila na hora. Suba para 64 na PRO 6000 de 96 GB. Na calibração (seção 5.0a), confira no `/metrics` se o uso de KV cache passa de 95% ou se há preempções. Se passar, reduza para 48 ou 32 e registre. Uma vez escolhido, o valor é fixo para todos os harnesses.
- Com MTP, o ganho da especulação cai quando o lote cresce: a GPU deixa de estar ociosa entre tokens. É por isso que a rampa mede tok/s e aceitação a cada degrau. Se a calibração mostrar que acima de certo N o MTP piora o agregado, registre como achado. Não desligue o MTP no meio: isso seria uma variante separada.
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
3. Cada exercício roda em contêiner próprio e descartável, com a cópia montada em `/work`, que também é o diretório de trabalho. Nada mais do disco do host fica montado, fora o cache de pacotes (só leitura) e a configuração do harness (só leitura). Assim, "só pode ler e escrever dentro da sua pasta" vale para qualquer harness, inclusive para comandos de shell. Sessão nova e contexto isolado por exercício: diretório de estado do harness vazio a cada exercício (seção 5.5 para o `fh`).
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

### 5.0a Paralelização entre exercícios: rampa até 20
O objetivo é fazer o conjunto inteiro terminar mais rápido, rodando vários exercícios ao mesmo tempo, sem estragar a comparação. Duas partes:

**1. Calibração (uma vez, antes do primeiro harness, fora da comparação).**
- Servidor recém-iniciado, com o `fh` no modo 1 agente e os primeiros 40 exercícios do catálogo. Essas execuções não contam e são descartadas.
- Degraus de concorrência de exercícios: 4, 8, 12, 16, 20. Cada degrau dura pelo menos 10 minutos ou 8 exercícios concluídos, o que vier por último. Um exercício novo só entra quando outro termina, para manter N constante no degrau.
- No fim de cada degrau, imprima e grave em `WORKDIR/calibracao.csv`:
  - tok/s de saída por fluxo: mediana e P10;
  - tok/s agregado: saída total ÷ tempo do degrau;
  - requisições rodando e em fila, média e pico;
  - uso de KV cache, média e pico, e preempções;
  - TTFT P50 e P90;
  - aceitação do MTP;
  - potência média da GPU;
  - erros e timeouts.
- **Critério de parada da rampa.** Pare de subir, e fique no último degrau bom como N_MAX, se no degrau seguinte acontecer qualquer um destes:
  - o agregado ganhar menos de 10% em relação ao degrau anterior;
  - o tok/s mediano por fluxo cair abaixo de 40% do medido com 4 em paralelo;
  - o KV cache passar de 95% de forma sustentada, ou aparecerem preempções;
  - o TTFT P90 passar de 30 s;
  - surgir qualquer erro 5xx ou falta de memória.

  Se nada disso acontecer, N_MAX = 20. Registre a decisão com os números no relatório: tabela e gráfico de tok/s por fluxo e agregado contra N.

**2. Execução principal (todos os harnesses, mesma escala).**
- A concorrência depende da posição do exercício no catálogo, não do harness:
  - exercícios 1–10: 4 em paralelo;
  - 11–20: 8;
  - 21–30: 12;
  - 31–40: 16;
  - 41–130: N_MAX, no máximo 20.

  Se N_MAX ficou abaixo de 20, os degraus acima dele são pulados. Assim cada exercício roda, em todos os harnesses, sob a mesma concorrência de exercícios, e a comparação exercício a exercício continua justa.
- Durante a execução, a cada degrau e depois a cada 10 exercícios concluídos, imprima a mesma linha de números da calibração, com o harness e o N atual, e grave-a em `WORKDIR/runs/<harness>/tps.csv`.
- Freio de segurança. Se, durante a execução de um harness, aparecer erro 5xx, falta de memória ou KV cache acima de 98% por mais de 2 minutos, pare de lançar exercícios novos até a fila esvaziar e depois continue no mesmo N. Nunca mude a configuração do servidor. Registre cada acionamento, com horário; é resultado do harness.
- O timeout continua 900 s de parede em todos os degraus. Se a calibração mostrar que o tok/s por fluxo em N_MAX cai para menos da metade do de N=4, reporte a taxa de timeout por degrau e mostre separadamente os exercícios que estouraram o tempo nos degraus altos.
- O modo máximo do `fh` soma workers aos exercícios paralelos: com 20 exercícios, pode haver bem mais de 20 requisições ao mesmo tempo. Não reduza o N desse modo; a disputa pela GPU é o custo do fan-out e aparece como fila no relatório.

### 5.1 OpenCode
Provedor OpenAI-compatível (`@ai-sdk/openai-compatible`) apontando para `http://<proxy>:8001/v1`, modelo `frankenstein-v2`, com a chave do exercício. Confira a sintaxe de provedor customizado e do modo não interativo (`opencode run`) na documentação da versão instalada. Use as mesmas permissões da rodada anterior: bash restrito a `scripts/test.sh`, `lint.sh`, `setup.sh`, `git status`/`git diff` e `pwd`; sem web, sem subagentes, sem skills. Desligue compartilhamento e atualização automática. Registre a configuração exata (sem a chave).

### 5.2 DeepSeek Harness
Localize o repositório oficial e verifique se aceita endpoint OpenAI-compatível customizado e tool calling com este modelo. Se não aceitar, marque UNSUPPORTED com a evidência (trecho de código ou documentação, versão) e siga. Se aceitar, use modo não interativo, com as mesmas permissões e sem web.

### 5.3 Qwen Code
Via `OPENAI_BASE_URL=http://<proxy>:8001/v1`, `OPENAI_API_KEY=<chave do exercício>` e `OPENAI_MODEL=frankenstein-v2`, em modo não interativo (`qwen -p "<prompt>"`, com aprovação automática de ferramentas: confira a flag na versão instalada). Sem web, sem MCP, telemetria desligada.

### 5.4 Claude Code
- **Endpoint.** Precisa de endpoint no formato da API da Anthropic. Verifique se o vLLM instalado expõe `/v1/messages`. Se não expuser, use um tradutor, por exemplo LiteLLM com rota `/v1/messages` → provedor `hosted_vllm` apontando para o proxy. A cadeia fica Claude Code → LiteLLM → proxy → vLLM, e o proxy continua sendo o ponto de medição. Registre a versão do LiteLLM.
- **Modelos.** Configure `ANTHROPIC_BASE_URL` para ele e aponte todos os slots de modelo para `frankenstein-v2`: `ANTHROPIC_MODEL`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL` e o slot "rápido" da versão instalada. Use a chave do exercício em `ANTHROPIC_AUTH_TOKEN` ou `ANTHROPIC_API_KEY`, conforme a versão.
- **Tráfego externo.** Defina `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` e `DISABLE_TELEMETRY=1`.
- **Execução.** Não interativo: `claude -p "<prompt>" --output-format json`, com permissões equivalentes às do OpenCode via `--allowedTools`/`--permission-mode`. Confira as flags com `claude --help`.
- **Prova.** Confirme no log do proxy e no firewall que nenhuma chamada saiu para a Anthropic.

### 5.5 Frankenstein Harness, modo 1 agente
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
fh run --auto --yes --mode yolo --keep --json --cwd /work "<prompt de tarefa>"
```

Pontos obrigatórios:
- **`FH_HOME` vazio por exercício**: `/fh-state` é um tmpfs do contêiner, fora de `/work`, para não aparecer no diff nem na correção. O `fh` aprende skills e guarda sessões ali. Compartilhar esse diretório entre exercícios contaminaria a comparação.
- **`--keep`**: por padrão o `fh` desfaz as mudanças quando a própria verificação reprova. Os outros harnesses sempre deixam o trabalho no disco. Com `--keep`, o grader vê o que o agente fez. Registre o `verdict` do `fh` (`pass`/`fail`/`unverified`) e quantas vezes o `fh` reprovou algo que o grader aprovou, e vice-versa. Essa é a medida de qualidade do verificador.
- **Sem `--sandbox`** na comparação principal: o contêiner já isola todos os harnesses igualmente.
- Guarde o JSON de `--json`: rodadas de verificação, checks executados, workers, tokens, tool calls, chamadas reparadas ou malformadas, aceitação do MTP.
- **Achado a observar no piloto:** o `fh` infere os comandos de verificação pelo tipo de projeto (`dotnet build/test`, `npm test` etc.). Ele não conhece `scripts/test.sh`, que aqui roda um programa de console e não um projeto de teste. Registre quais checks ele rodou e se algum falhou por comando errado. Não mude o `fh` durante a execução (regra 8).

### 5.6 Frankenstein Harness, modo máximo de agentes
Mesmo comando, com `maxConcurrency: 16` (o teto de workers por exercício, independente do `max-num-seqs`). Isso não força 16 agentes. É o teto: o planejador divide a tarefa em até 16 subtarefas com arquivos disjuntos, só quando elas são independentes. O regulador começa com 2 simultâneos e sobe conforme o `/metrics`. Também podem surgir workers "extra" para arquivos que ninguém possuía e um corretor por diretório em rodadas de reparo. Todos ficam restritos à pasta, pelo contêiner.

Registre por exercício:
- subtarefas planejadas;
- workers criados, incluindo os extra;
- tokens por worker;
- tempo em fila no servidor (`num_requests_waiting` e o histograma de fila no intervalo do exercício).

Com `max-num-seqs` fixo, esse modo pode saturar; isso é resultado, não erro. Registre também em quantos exercícios o planejador não dividiu nada, porque nesses exercícios os dois modos são equivalentes.

## 6. Fase 5: o que medir (por exercício e agregado por harness)
- **Qualidade:**
  - aprovado, nota (score/maximum), critérios aprovados por nível (mínimo, esperado, excelente);
  - erros de infraestrutura (fora do denominador, listados), timeouts;
  - o `verdict` do próprio harness contra o grader (só para o `fh`).
- **Tempo:**
  - tempo de parede total do harness;
  - por exercício: média, mediana, P90;
  - tempo até o primeiro token: mediana e P90 por requisição.
- **Tokens:**
  - entrada, cache, entrada sem cache, saída e raciocínio (estimado);
  - total por exercício e por harness;
  - tokens por exercício aprovado, total e sem cache.
- **Velocidade por degrau de concorrência (4, 8, 12, 16, 20):**
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
  - agentes por exercício (Frankenstein Harness);
  - tentativas de rede bloqueadas.
- **Hardware:**
  - uso médio e de pico de GPU, VRAM, potência média, temperatura;
  - energia por harness (integral da potência no tempo) e energia por exercício aprovado.

## 7. Fase 6: análise e relatório

### Análise
- **Ranking.** Taxa de aprovação com intervalo de Wilson de 95%.
- **Comparação pareada.** Exercício a exercício, entre cada par de harnesses: teste binomial exato bicaudal nos pares discordantes (McNemar exato) e intervalo de 95% da diferença de taxas por bootstrap pareado (10 000 reamostragens, semente fixa registrada). Corrija as comparações múltiplas por Holm. Não diga "melhor" sem apoio estatístico; use "sem diferença conclusiva" quando for o caso. Com 130 exercícios, diferenças de poucos pontos quase nunca são conclusivas; diga isso.
- **Recortes.** Resultados por trilha, stack (.NET, React, Angular) e nível.
- **Por tipo.** Exercícios cujo código inicial devolve `<unimplemented>` (contrato de saída implícito) contra os que já têm código real.
- **Escala.** O que a paralelização entre exercícios rendeu:
  - tempo total de parede real contra o estimado se todos tivessem rodado com 4 em paralelo (soma das durações ÷ 4);
  - tok/s agregado e por fluxo por degrau;
  - onde a curva achata e o motivo: KV cache, fila ou queda da aceitação do MTP.
  - Diga também se a aprovação mudou entre degraus. Como os degraus seguem a ordem do catálogo, qualquer diferença mistura dificuldade com concorrência. Aponte a confusão e não conclua causalidade.
- **1 agente contra máximo.** Aplique a regra de fan-out do projeto (`docs/FANOUT.md`), que vale para exercícios em que houve divisão. O modo máximo compensa se não perder qualidade e se: (a) a mediana de tempo ficar ≤ 80% da do modo 1 agente, ou a aprovação subir pelo menos 5 pontos; e (b) os tokens sem cache por exercício aprovado ficarem ≤ 2× os do modo 1 agente. Mostre também o resultado para todos os 130.
- **Com repetições.** Se houver repetições, reporte a média por exercício e a variância entre elas. As estatísticas pareadas usam a taxa média por exercício.
- **Referência externa**, marcada como ambiente diferente e fora das estatísticas: a rodada anterior do Frankenstein V2 com harness caseiro (57,7%, temperatura 0,2) e os resultados do Claude Opus 5.5 (50,0%) e do Sonnet 5.5 (47,7%) no Claude Code.

### Avaliação independente das instruções e dos testes
Só o operador lê os graders, e só depois das execuções. Por exercício, avalie:
- a clareza do enunciado;
- se o formato de saída está especificado ou só implícito;
- se o teste público verifica o que o grader privado exige (liste os critérios privados sem equivalente público);
- ambiguidades que forçam adivinhação;
- testes instáveis: rode o grader 3 vezes na referência e nos 5 exercícios com mais discordância entre harnesses;
- se a solução de referência passa no próprio grader (fase 0).

Use também o sinal empírico: critérios que nenhum harness passou e critérios que falham só por formato. Termine com uma classificação (claro, ambíguo, defeituoso) e sugestões de correção por exercício.

### Entregáveis em `WORKDIR/relatorio/`
1. `relatorio.md` e `relatorio.txt` com toda a análise e todos os números. Primeira seção: resumo de uma página com o ranking, a comparação 1 agente × máximo e a lista de NOT_RUN/UNSUPPORTED.
2. `relatorio.pdf` com tabelas e gráficos: ranking com intervalos, tokens, tempo e velocidade por harness, aprovação por trilha/stack/nível, tok/s por fluxo e agregado contra o número de exercícios em paralelo (calibração e cada harness), e 1 agente × máximo.
3. `results.csv` e `results.json` por exercício e harness, os logs do proxy, do `/metrics` e do `nvidia-smi`, e os JSONs do grader.
4. `MANIFEST.json`:
   - commits (jg-eng-tests e `fh`);
   - versões (vLLM, CUDA, driver, cada harness, LiteLLM se usado, .NET, Node, pwsh, language servers, digest da imagem);
   - o comando exato do servidor e as flags de fallback usadas;
   - amostragem imposta, `max-model-len`, `max-num-seqs` final, N_MAX e a escala de rampa usada;
   - método de identificação por harness, regra de firewall;
   - horários de início e fim de cada fase e de cada harness.

Ao atingir qualquer limite (tempo, disco, fase em EXECUTAR_ATE), gere o relatório com o que foi medido e liste o que ficou NOT_RUN ou UNSUPPORTED, com o motivo.

# Sessão separada: as onze configurações no repositório ai-jail (e avaliação do próprio conjunto de testes)

Documento autônomo, executado por outra LLM numa **sessão própria**, depois (ou fora) da sessão principal (`docs/VALIDACAO_JG_ENG_TESTS_V2.md`). Ele **herda a mecânica** do plano V2 e só define o que é específico do ai-jail. Onde este documento calar, vale o plano V2 (e a seção "Decisões do dono" dele vale acima de tudo, incluindo "só vale o resultado final", "o juiz é a LLM operadora, nunca o Qwen" e **"nenhum teste é pulado"**).

Repositório avaliado: https://github.com/akitaonrails/ai-jail (Rust, licença GPL-3.0). Fixe o commit do HEAD no início e registre. **Licença:** não copie conteúdo do ai-jail para nossos repositórios nem para o relatório além de trechos curtos de citação; as tarefas ficam só em WORKDIR e não são commitadas em lugar nenhum.

## 1. Objetivo
1. Rodar **as mesmas onze configurações** (`qwen27b-direto`, `fh-nomem`, `fh-mem1`, `fh-memmax`, `opencode`, `qwen-code`, `claude-code`, `deepseek`, mais `mimo9b-direto`, `qwen27b-mimo9b-puro` e `qwen27b-mimo9b-fh`; na ordem da decisão 7 do plano V2: sem harness, depois `fh`, depois os outros) em tarefas reais extraídas da história do ai-jail, e comparar o efeito de cada harness sobre o modelo fixo `nvidia/Qwen3.8-27B-NVFP4` (MTP 3).
2. **Avaliar o próprio conjunto**: dizer quais tarefas e testes são confiáveis (determinísticos, sem dependência escondida do ambiente, sem ambiguidade), para que a nota signifique algo.

Entregáveis numa pasta separada `relatorio-ai-jail/` (nunca misturada à do jg-eng-tests nem à do DeepSWE).

## 2. Independência da sessão principal
- Servidores vLLM **próprios** desta sessão (Qwen3.8-27B e o MiMo-9B da seção 2.2 do plano V2, com o MTP já escolhido na sessão principal; se a varredura não foi feita, faça-a antes), recém-iniciado, mesmo comando e mesmas flags da seção 2.1 do plano V2. **Nunca** simultâneo com outra sessão nem com a fase 8 (capacidade) do plano principal: as duas dividiriam a GPU e contaminariam medidas.
- Fases 0 a 3 do plano V2 (máquina, downloads, runtime de contêiner, servidor, proxy de medição, isolamento): reaproveite se a máquina já estiver pronta e o servidor verificado (122,1 tok/s por fluxo e 75,7% de aceitação MTP, margem de 15%); caso contrário, refaça-as.
- Memória do `fh`: **vazia** no início de cada configuração do `fh` nesta sessão (não herda a das outras sessões).
- Concorrência: reaproveite o **N_MAX** da calibração da sessão principal se ela existir e o hardware for o mesmo; senão rode a rampa de 5 em 5 da seção 5.0a (aqui basta a rampa, sem o teste de capacidade). Repartição justa e cota por configuração: seção 5.0 e item 8 da seção 3 do plano V2.

## 3. Mineração das tarefas (ferramenta: `integrations/ai-jail/mine_commit_tasks.py`)
O ai-jail não traz uma suíte de tarefas pronta; as tarefas saem dos commits (estilo SWE-bench): **repositório no commit pai** de uma correção/funcionalidade + **instrução** escrita por você + **testes ocultos** do commit.

1. Prepare: `git clone` do ai-jail em `WORKDIR/ai-jail-src`; `python3 integrations/ai-jail/tests/test_mine_commit_tasks.py` precisa passar antes.
2. `mine_commit_tasks.py list --repo <src> --kinds fix,feat,perf,refactor > candidatos.json`. Na amostra feita no desenvolvimento saíram **86 candidatos**. **Todos** são construídos e validados (nada é cortado por tempo; decisão 14).
3. `build` e `validate` de cada candidato, com `CARGO_TARGET_DIR` compartilhado só **dentro da validação** (nunca dentro dos contêineres dos agentes). A validação segue as regras:
   - Executa primeiro a **referência** (o commit): R = testes que **rodaram e passaram** no ambiente de correção.
   - Executa a **base** (commit pai + testes ocultos) sobre R: ao menos um teste de R falha (`fail_to_pass`); o resto é `pass_to_pass`.
   - Tarefa **válida** = há `fail_to_pass` e a referência passa tudo de R. Do contrário, fica **inválida com o motivo registrado** (não rodou, depende de plataforma, precisa de `bwrap`/userns, não falha na base, falha na referência). Tarefa inválida **não entra na nota**, mas aparece no relatório com a evidência: isso não é pular teste, é a avaliação do conjunto (objetivo 2).
   - Rejeite com evidência as tarefas implausíveis (por exemplo, um commit que muda 157 testes de uma vez: a instrução não consegue descrever o comportamento exigido).
4. Ambiente de **correção** (grader), no host ou num contêiner privilegiado do operador: Rust **1.97.1** (o `rust-toolchain.toml` do projeto fixa), `bubblewrap` instalado em local confiável e **user namespaces habilitados**, `cargo test --features test-hooks`. Sem o `bwrap`, um teste falha e envenena um mutex, o que derruba 92 testes em cascata (`PoisonError`): verifique `bwrap --version` e a execução da suíte inteira no HEAD limpo **3 vezes** antes de começar, e registre o resultado (base do objetivo 2).
5. Ambiente do **agente**: contêiner por tarefa e por configuração com o toolchain Rust 1.97.1 pré-instalado, dependências já buscadas (`cargo fetch`/`vendor` no momento da construção da imagem, para rodar sem internet), a árvore de trabalho = o commit pai (`workspace/` da tarefa), sem `.git` do futuro e sem os testes ocultos. Contêineres de agente em geral **não criam user namespaces**: testes que dependem de `bwrap` não rodam ali, e é por isso que a nota vem do grader do operador, nunca do que o agente conseguiu rodar.
6. **Instrução** de cada tarefa (a única coisa escrita por você): texto no estilo de issue, em inglês (como o repositório), que descreve o sintoma ou a funcionalidade **sem vazar o diff nem os nomes dos testes**. Ela precisa listar, em linhas separadas "Interface", os símbolos públicos que os testes ocultos usam e que não existem na base (saem de `interface_hints` no `task.json`, obtidos dos erros de compilação na base), senão uma solução correta não compila contra os testes. Revise cada instrução num segundo passe: um leitor sem acesso ao diff consegue implementar a partir dela? Se não, reescreva. Instruções são idênticas para as onze configurações.
7. Salve em `WORKDIR/ai-jail-tasks/<sha>/` (`task.json`, `hidden/`, `workspace/`, `instruction.md`, `validation.json`). Ordem de execução: a da história do git (mais antigo primeiro), igual para todas.

## 4. Execução
- Reuse a mecânica do plano V2: seção 5.0 (isolamento e orquestrador com retomada pelo `state.json`), 5.0a (G e rampa), 5.0b (julgamento), 5.0c (passe de tempo: cada configuração sozinha, em 20 tarefas), 5.1 (modos do `fh`), 5.2 (`qwen27b-direto` via `fh direct`: um pedido com o snapshot do repositório, blocos SEARCH/REPLACE aplicados como escritos) e 5.3 a 5.6 (os outros harnesses, com a mesma amostragem imposta pelo proxy).
- Prompt de tarefa: idêntico nas onze configurações = instrução + "Trabalhe no repositório do diretório atual; só vale o estado final do disco. Não há acesso à internet." Timeout por tarefa: **1 800 s** (compilação Rust é lenta; os 900 s do jg-eng-tests não servem). Amostragem: temperatura 1,0, top_p 0,95, top_k 20, imposta pelo proxy.
- Os agentes veem só `workspace/`. Nunca `hidden/`, `validation.json` nem as tarefas das outras configurações.

## 5. Correção e julgamento (só a LLM operadora; nunca o Qwen)
1. Grader objetivo: `mine_commit_tasks.py grade <tarefa> <workspace-do-agente>`. Sobrepõe os testes ocultos (os `tests/*.rs` e o módulo `#[cfg(test)]` do commit **substituindo** o do candidato) e executa. **Passa** quando todos os testes de R rodaram e passaram. Resultado "teste ausente/não rodou" = falha (o agente não pode esconder um teste apagando-o).
2. Mesmo ambiente de correção para as onze, o mesmo `CARGO_TARGET_DIR` **jamais** compartilhado entre candidatos de configurações diferentes sem tocar nos mtimes (`mine_commit_tasks.py` já usa cópias com mtime novo e `CARGO_INCREMENTAL=0`; não contorne isso).
3. **Revisão às cegas** (5.0b do plano V2): você lê cada diff anonimizado, sem saber a configuração, e dá nota de 0 a 2 em: correção do comportamento pedido, ausência de regressão/gambiarra para passar teste, qualidade. Critério extra **específico do ai-jail**: *a mudança não pode enfraquecer a segurança do sandbox* (relaxar restrições do bwrap/landlock/seccomp, ampliar montagens, ignorar erros de isolamento, `unsafe` novo sem justificativa, trocar uma falha fechada por uma aberta). Qualquer enfraquecimento = reprovado, mesmo com os testes passando.
4. Reporte por configuração: aprovadas/válidas com intervalo de Wilson 95%, McNemar pareado contra `qwen27b-direto` e entre as configurações da cadeia (direto → nomem → mem1 → memmax), tokens, requisições, tempo de parede, tok/s do passe de tempo. A ordem é a da decisão 7 do plano V2. "Só vale o resultado final" vale aqui também.

## 6. Avaliar o conjunto (o "eval it too")
Registre em `relatorio-ai-jail/avaliacao-do-conjunto.md`:
1. **Determinismo:** a suíte inteira no HEAD limpo e a de cada tarefa válida na referência, **3 execuções** cada; lista de testes instáveis (e a causa), com ordem aleatória e sem ela.
2. **Dependências do ambiente:** testes que exigem `bwrap`, user namespaces, rede, `$HOME` específico, sistema operacional (cfg(target_os)) ou usuário; o efeito do `PoisonError` em cascata e se algum teste mascara outro.
3. **Qualidade das tarefas:** quantas das 86 candidatas são válidas e por que as outras não são; ambiguidade (a instrução basta?); testes que verificam detalhe de implementação em vez de comportamento; tarefas triviais (todas as dez passam) ou impossíveis (nenhuma passa, nem com a solução de referência revisada).
4. **Sensibilidade:** quais tarefas separam as configurações; margem de erro com o N válido.
5. **Cobertura do que a nota diz:** áreas do ai-jail (sandbox, config, CLI, status/relatórios) sub- ou super-representadas.
6. Problemas do próprio ai-jail encontrados durante a avaliação (bug real, teste frágil): anote como achado, sem alterar o repositório.

## 7. Entregáveis (em `relatorio-ai-jail/`)
- `relatorio.md` (e `.pdf`): resumo executivo, tabela por configuração (ordem da decisão 7), cadeia de ablação, ressalvas.
- `resultados.csv`/`resultados.json`: uma linha por (tarefa, configuração).
- `avaliacao-do-conjunto.md` (seção 6), `tarefas/` (apenas `validation.json` e metadados; **sem** conteúdo do ai-jail), `MANIFEST.json` (commit do ai-jail, do `fh`, versões de todas as ferramentas, comando do vLLM, SHA-256 dos artefatos).
- `PROGRESSO.md` e `state.json` para retomar.

## 8. Tempo (estimativa, não limite)
Construção e validação das 86 candidatas: ~2 a 4 h (compilação Rust). Execução: 11 configurações × (tarefas válidas, ~50 a 70) ≈ 400 a 560 execuções de até 30 min; com N_MAX 15 a 20, ~8 a 14 h. Passe de tempo ~1 h. Informe o dono do tempo restante a cada etapa; só ele decide cortar algo.

## 9. Observação sobre o escopo
Aqui o ai-jail é o **repositório de teste** das onze configurações. A leitura alternativa (rodar os harnesses *dentro* do sandbox do ai-jail, medindo o custo de isolamento) **não está neste plano**; se o dono quiser, vira uma sessão adicional.

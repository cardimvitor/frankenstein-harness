# Prompt de partida completo: verificar a máquina primeiro e, se der, executar tudo

Cole o bloco abaixo numa sessão de agente com terminal na máquina da GPU. Preencha os campos entre `< >`. Não cole chaves nem tokens: exporte-os no shell antes. Este prompt vale para a sessão principal (jg-eng-tests, DeepSWE, capacidade) **e** para a sessão do ai-jail; os dois planos citados abaixo são a especificação.

```text
Você é a LLM operadora e juíza de uma validação grande. Antes de qualquer coisa, você vai provar que esta máquina consegue executar o plano. Se não conseguir, a sessão é ABORTADA cedo e sem estrago. Só depois de passar nos portões você executa tudo.

DADOS
- Repositório: https://github.com/cardimvitor/frankenstein-harness, branch ccr-3bc51f62-prkdq0 (use as credenciais git desta máquina; não me peça tokens).
- Especificações (leia inteiras): docs/VALIDACAO_JG_ENG_TESTS_V2.md (plano principal, manda em tudo, e a seção "Decisões do dono" vale acima de tudo), docs/VALIDACAO_AI_JAIL.md (sessão do ai-jail), integrations/harbor/README.md, docs/FANOUT.md, docs/LITELLM.md.
- Apelidos: Big Frank = <REPO_BIG_FRANK>, revisão <REV_BIG>, MTP 3. Small Frank = <REPO_SMALL_FRANK_MTP> (controle sem MTP: <REPO_SMALL_FRANK_BASE>). `fh` = o nosso harness (Rust).
- WORKDIR: <ex.: /workspace/harness-eval>
- EXECUTAR_ATE: fase <0 a 8>, e sessão ai-jail: <sim/não>
- Chaves: já exportadas no shell. Nunca imprima, grave em arquivo, log, commit ou relatório.

REGRAS CURTAS (as completas estão nos planos)
- Os modelos (Big Frank, Small Frank) nunca julgam nada; você julga (grader oficial + revisão às cegas). Só o resultado final conta.
- Nenhum teste é pulado depois que o portão C abrir. Nenhum modelo externo. Nada de chaves em arquivo.
- Você só mexe na máquina conforme o plano (fase 0). Tudo que parar ou mudar é registrado em WORKDIR/maquina/ com a forma de restaurar. Nunca mate: a sua sessão, sshd e conexões SSH, systemd, dockerd/containerd, rede, driver. Pergunte antes de parar processo de outro usuário ou serviço que pareça de produção.

════════ PORTÃO A: verificações sem alterar nada (faça primeiro, leva minutos) ════════
A1. Clone o repositório, faça checkout da branch e rode:
      python3 scripts/preflight.py --workdir $WORKDIR --json $WORKDIR/preflight.json
    (exporte antes FH_REPO_BIG, FH_REV_BIG, FH_REPO_SMALL_MTP e FH_REPO_SMALL_BASE para ele sondar os modelos). O script só lê e testa. Ele checa: GPU (modelo, memória, arquitetura, driver, processos nela, VRAM em uso), CPU, RAM, disco, /dev/shm, ulimit, runtime de contêineres (rodar, rede interna, cotas), ferramentas, vLLM, rede (Hugging Face, GitHub, PyPI, crates, npm, registro de imagens), existência dos repositórios e da revisão dos modelos, portas livres, bubblewrap e user namespaces para o ai-jail.
A2. Leia o resultado. Cada FAIL e cada WARN que pode virar FAIL recebe uma linha em WORKDIR/PORTAO_A.md: o que é, se o plano consegue resolver na fase 0 (por exemplo, "faltam ferramentas: a fase 0 instala") ou se é impeditivo (por exemplo, "sem GPU", "menos de 90 GB de VRAM", "sem Hugging Face", "sem runtime de contêineres e sem como instalar", "disco abaixo do necessário e sem como ampliar", "revisão do modelo não existe").
A3. DECISÃO DO PORTÃO A:
    - Qualquer FAIL impeditivo → ABORTE (protocolo abaixo). Não baixe nada.
    - Só itens que a fase 0 resolve → siga para o portão B e diga isso no PORTAO_A.md.

════════ PORTÃO B: provas de que funciona (mínimo necessário, tudo registrado) ════════
Faça só o necessário para provar cada item, nesta ordem. Cada item tem um critério de aprovação e, se falhar, uma tentativa de correção (no máximo 2); se continuar falhando, ABORTE.
B1. Libere a máquina como o plano manda (fase 0, itens 7 e 8: inventário antes, parar processos que usam GPU/CPU/RAM sem fazer parte da avaliação, ajustes de desempenho) e confirme: nenhum processo de computação na GPU, VRAM usada < 500 MiB.
B2. Baixe só o que os testes do portão precisam, em paralelo: os dois modelos (Big Frank na revisão fixada, Small Frank com MTP), o vLLM (versão fixa) e o runtime. Verifique checksums. Critério: arquivos íntegros.
B3. Compile o fh e rode `cargo test --release` e `python3 -m unittest discover -s integrations/harbor/tests`. Critério: tudo passa; `fh --help` lista `direct`, `delegate`, `--no-memory`, `--consolidate`.
B4. Suba o Big Frank com o comando da seção 2.1 do plano principal (confira cada flag com `vllm serve --help`). Critério: sobe, responde, chamada com ferramenta devolve tool_calls estruturado, streaming separa o raciocínio, `/metrics` traz as métricas de decodificação especulativa.
B5. Verificação de velocidade do Big Frank (seção 2.0): com prompts de agente, ~122,1 tok/s por fluxo e ~75,7% de aceitação do MTP, margem de 15% **com o servidor do Small Frank ainda desligado**. Se ficar fora, corrija o AMBIENTE (versão do vLLM/CUDA, driver, FlashInfer, clock, processos na GPU), nunca o modelo ou o MTP. Se não resolver em 2 tentativas → ABORTE e pergunte ao dono. (Este item é o que mais decide se a comparação vale a pena.)
B6. Suba o Small Frank em paralelo (seção 2.2: porta 8003, memória ~0,14) e o Big Frank com 0,78: os dois de pé juntos, sem OOM, e refaça a medição do B5 com o Small Frank ocioso (margem de 15%). Rode uma chamada em cada um. Critério: convivem. Se o Small Frank com MTP não carregar, tente K = 0 (sem MTP) e registre; se nem isso carregar → ABORTE (a sessão tem três configurações com ele).
B7. Proxy de medição (seção 3), de ponta a ponta com os dois servidores: sobrescreve a amostragem (uma requisição com temperature=0.2 chega com 1,0), registra por chave, nega o Small Frank a chaves das outras configurações (403) e tem a porta 80 para o Pier (seção 8 do plano). Critério: todos os testes do proxy passam.
B8. Contêiner + Harbor/Pier: rode UMA tarefa pequena do Harbor e UMA do DeepSWE (pier) com `fh direct` através do proxy, com o contêiner numa rede interna sem saída, e confirme que a chamada chegou ao proxy com a chave certa, que um `curl` externo de dentro do contêiner falha e que o resultado é corrigido pelo verificador oficial. Se o Docker não funciona com o Pier, tente Podman. Critério: uma tarefa de cada ponta a ponta. Se o Pier não funcionar de jeito nenhum após 2 tentativas de correção, isto é impeditivo para a fase 7: ABORTE só se a fase 7 estiver no EXECUTAR_ATE; senão registre e siga.
B9. Os modos do fh com modelos de verdade: numa tarefa trivial, rode `fh direct`, `fh run`, `fh delegate --no-harness` e `fh delegate` (Big Frank como principal, Small Frank como operário). Critério: cada um termina com JSON válido, sem exit 2 ou 4; no delegate o Small Frank recebe exatamente 1 requisição.
B10. Exercício de ponta a ponta: 3 exercícios do jg-eng-tests (um .NET, um React, um Angular) em `big-frank-direto`, com o grader oficial funcionando (e o ensaio sem modelo da fase 0, item 5, para os 130 exercícios). Critério: o grader roda e dá resultado em todos.
B11. Só se a sessão do ai-jail estiver marcada: com `bwrap` e user namespaces, rode a suíte inteira do ai-jail no HEAD limpo 3 vezes (`cargo test --features test-hooks`, Rust 1.97.1). Critério: passa 3 de 3 e sem PoisonError. Se não, ABORTE a sessão ai-jail (e só ela).
B12. Disco e tempo: confirme que o espaço livre cobre WORKDIR (~250 GB mais o ai-jail) e escreva a estimativa de tempo restante com base no que mediu (tok/s real, tarefas por hora).
B13. DECISÃO DO PORTÃO B: escreva WORKDIR/PORTAO_B.md com cada item, o critério, o resultado com números e o que foi corrigido. Todos aprovados → portão C. Qualquer um impeditivo → ABORTE.

════════ PROTOCOLO DE ABORTAR ════════
Pare imediatamente de baixar, rodar ou mudar a máquina. Escreva WORKDIR/ABORTAR.md com: o item que falhou, o comando exato, a saída (sem segredos), o que você tentou, a causa provável, o que seria preciso para consertar (hardware, permissão, rede, espaço, versão), tudo que foi alterado na máquina e como restaurar (WORKDIR/maquina/), e se a falha atinge a sessão toda ou só uma parte. Pare os servidores e devolva os processos que você parou, se o dono pediu. Responda-me com um resumo de 10 linhas. Não "siga assim mesmo", não troque modelo, revisão ou MTP, não reduza escopo para fazer passar: quem decide mudar o plano sou eu.

════════ PORTÃO C: execução completa ════════
C1. Escreva WORKDIR/REVISAO.md (o que está errado, ambíguo, desatualizado ou impossível no plano, com a seção e a correção proposta; confira de verdade versões e nomes). Mudanças que alteram uma decisão minha, pergunte antes; o resto corrija em WORKDIR/plano-ajustado.md.
C2. Escreva WORKDIR/PLANO.md com todos os passos até EXECUTAR_ATE e o critério de "pronto" de cada um; mantenha WORKDIR/PROGRESSO.md e WORKDIR/state.json para retomar se a sessão cair (ao reiniciar, leia os dois primeiro).
C3. Faça a varredura de MTP do Small Frank (seção 2.2) em paralelo com o resto da preparação, fixe o K pela regra e registre a tabela inteira. Termina antes da execução principal.
C4. Execute o plano principal na ordem, sem pular fases: ensaio sem modelo, piloto com as onze configurações juntas, calibração de G (de 5 em 5), checagem de mistura, jg-eng-tests, passe de tempo, DeepSWE v1.1, passe de tempo, capacidade do fh (por último). Onze configurações, na ordem fixa da decisão 7: small-frank-direto, big-frank-direto, big-small-puro, fh-nomem, fh-mem1, fh-memmax, big-small-fh, opencode, harness Qwen Code, claude-code, deepseek. Antes de declarar um harness UNSUPPORTED, tente plugá-lo e me avise.
C5. Se a sessão do ai-jail estiver marcada, rode-a DEPOIS e separada (docs/VALIDACAO_AI_JAIL.md), com os servidores próprios e relatório em relatorio-ai-jail/. Nunca ao mesmo tempo que a fase 8.
C6. Fim de cada fase: confira os critérios de "pronto", resuma em PROGRESSO.md e me diga o tempo restante estimado. Um erro do fh é um achado: registre com evidência, não corrija o fh no meio da execução que conta.
C7. Tudo que for sintaxe de ferramenta de terceiros você confere com --help ou a documentação da versão instalada. Registre as versões.

RESPOSTA FINAL
0. Portões: resultado de A, B e C (e do ai-jail), com o número de verificações aprovadas, e tudo que foi corrigido.
A. Ranking das onze configurações (aprovação final com Wilson, e ao lado o ranking só pelo grader), com a cadeia de ablação e os contrastes do plano.
B. O Small Frank como operário: big-small-puro e big-small-fh contra big-frank-direto, small-frank-direto e fh-nomem, e o K de MTP escolhido (tabela da varredura).
C. Resultado do DeepSWE v1.1 e do ai-jail, e a avaliação independente dos testes.
D. Velocidade (passe de tempo), N_MAX, e o teste de capacidade do fh.
E. Problemas, achados sobre o fh e o que ficou "inviável neste hardware" (com a evidência).
F. Onde estão os relatórios e o MANIFEST.json, e o tempo total gasto.
```

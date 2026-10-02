# Prompt de partida: sessão ai-jail

Cole o bloco numa sessão de agente com terminal na máquina da GPU. Não cole chaves; exporte-as antes.

```text
Você vai conduzir, do começo ao fim, uma sessão separada de validação: as onze configurações (big-frank-direto, fh-nomem, fh-mem1, fh-memmax, opencode, qwen-code, claude-code, deepseek, small-frank-direto, big-small-puro e big-small-fh) rodando tarefas extraídas do repositório https://github.com/akitaonrails/ai-jail, e a avaliação do próprio conjunto de testes. Você é a LLM operadora e juíza; o Qwen nunca julga nada.

DADOS
- Harness: https://github.com/cardimvitor/frankenstein-harness, branch ccr-3bc51f62-prkdq0 (use as credenciais git desta máquina). Especificação desta sessão: docs/VALIDACAO_AI_JAIL.md. Ela herda a mecânica de docs/VALIDACAO_JG_ENG_TESTS_V2.md; onde calar, vale o V2, e a seção "Decisões do dono" dele vale acima de tudo (só o resultado final conta; o juiz é você; nenhum teste é pulado).
- GPU: RTX PRO 6000 Blackwell 96 GB, inteira para esta sessão. Nunca rode junto com outra sessão de avaliação nem com a fase 8 do plano principal.
- WORKDIR: <ex.: /workspace/harness-eval>   (relatório em WORKDIR/relatorio-ai-jail/)
- Modelo fixo: nvidia/Qwen3.8-27B-NVFP4, revisão dbb8f445, MTP 3 (seção 2.0/2.1 do V2).

COMO TRABALHAR
1. Clone o repositório do harness, leia docs/VALIDACAO_AI_JAIL.md e as seções do V2 que ele cita (fases 0 a 3, 5.0 a 5.6, 6, 7) por inteiro. Leia também integrations/ai-jail/mine_commit_tasks.py e integrations/harbor/README.md.
2. Se a máquina ainda não está preparada, faça as fases 0 a 3 do V2 (libere GPU e recursos, baixe tudo em paralelo, runtime Docker senão Podman, vLLM com o modelo fixo e verificação de 122,1 tok/s por fluxo e 75,7% MTP, proxy de medição, isolamento). Se já está, só reverifique.
3. Escreva WORKDIR/REVISAO.md com o que estiver errado, ambíguo ou impossível em docs/VALIDACAO_AI_JAIL.md (confira de verdade: o HEAD do ai-jail, o rust-toolchain, se bwrap e user namespaces funcionam neste host, se `mine_commit_tasks.py` e seus testes rodam). Mudanças que alteram uma decisão minha, pergunte antes.
4. Minere, construa e valide todas as candidatas (seção 3 do documento); você escreve as instruções no estilo de issue, sem vazar o diff, com as linhas "Interface"; registre as tarefas inválidas com o motivo.
5. Rode a suíte limpa 3 vezes no HEAD (determinismo) antes de começar; sem bwrap/userns o grader está errado e você corrige o ambiente antes.
6. Calibre (ou reaproveite) o N_MAX, rode as onze configurações em paralelo e isoladas, na ordem da decisão 7 (sem harness, fh, outros), depois o passe de tempo.
7. Corrija com o grader do operador, faça a revisão às cegas (com o critério extra de não enfraquecer a segurança do sandbox) e escreva os entregáveis da seção 7, inclusive avaliacao-do-conjunto.md.
8. Mantenha PROGRESSO.md e state.json atualizados para retomar. Nunca imprima nem grave chaves. Não invente resultados; um resultado ruim do fh é um achado. Não altere o fh durante a execução que conta. Não copie conteúdo do ai-jail (GPL-3.0) para repositório nenhum.
9. Ao fim, responda com: resumo por configuração (ordem da decisão 7), cadeia de ablação, resultado da avaliação do conjunto, problemas, e o tempo gasto.
```

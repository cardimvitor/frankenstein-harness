# Prompt de partida: entregar a validação a outra LLM

Cole o bloco abaixo numa sessão de agente com terminal na máquina da GPU (por exemplo, Claude Code). Preencha os campos entre `< >` antes. Não cole chaves nem tokens no prompt: exporte-os no shell antes de abrir a sessão.

```text
Você vai revisar o meu plano de validação e depois conduzi-lo do começo ao fim. Você é a LLM operadora e juíza: revisa, configura, executa, mede e julga. O modelo avaliado (Frankenstein V2, um Qwen3.8-27B servido pelo meu vLLM nesta máquina) nunca julga nada.

DADOS
- Repositório do nosso harness: https://github.com/cardimvitor/frankenstein-harness, branch ccr-3bc51f62-prkdq0. Use as credenciais git que já existem nesta máquina; não me peça tokens.
- Documento a seguir: docs/VALIDACAO_JG_ENG_TESTS_V2.md nesse repositório. Ele é a especificação. Onde este prompt e o documento divergirem, vale o documento, e a seção "Decisões do dono do projeto" dele vale acima de tudo.
- GPU: uma NVIDIA RTX PRO 6000 Blackwell, 96 GB (SM120). É a única GPU: ela serve o modelo avaliado e mais nada.
- WORKDIR: <ex.: /workspace/harness-eval>
- EXECUTAR_ATE: fase <0 a 8> (7 = benchmarks públicos; 8 = teste de capacidade do fh, sempre por último)
- Segredos: qualquer chave já está exportada no shell. Nunca a imprima, grave em arquivo, log, commit ou relatório.

COMO TRABALHAR
1. Clone o repositório em WORKDIR/frankenstein-harness, faça checkout da branch e leia docs/VALIDACAO_JG_ENG_TESTS_V2.md inteiro antes de executar qualquer coisa. Leia também README.md, docs/FANOUT.md, docs/LITELLM.md e integrations/harbor/README.md, porque o documento se refere a eles.
2. Revise o plano antes de executar. Escreva WORKDIR/REVISAO.md com:
   - tudo que estiver errado, ambíguo, contraditório, desatualizado ou impossível nesta máquina, com a seção do documento e a correção proposta;
   - o que falta para cumprir as decisões do dono;
   - os riscos de tempo, disco e GPU.

   Confira de verdade, não só lendo: versões e nomes de datasets (Harbor Hub, repositórios dos benchmarks), existência dos checkpoints no Hugging Face, flags do vLLM instalado, e se os scripts e adaptadores do repositório rodam (`cargo test --release`, `python3 -m py_compile integrations/harbor/fh_harbor/*.py`). Corrija o que for claramente erro de digitação ou de comando na sua cópia de trabalho do plano (WORKDIR/plano-ajustado.md) e anote cada mudança. Mudanças que alteram uma decisão minha, ou que mudam o que é comparado, você me pergunta antes. Depois siga.
3. Configure a melhor combinação possível para esta GPU (decisão 8 e seção 2.0 do documento). Baixe os checkpoints oficiais do Qwen3.8-27B próprios para o Blackwell:
   - NVFP4 da NVIDIA (`nvidia/Qwen3.8-27B-NVFP4`) e as outras quantizações fiéis listadas;
   - o FP8 oficial, se existir;
   - o BF16 como referência de qualidade.

   Ajuste o vLLM (MTP, KV cache, memória, lote, contexto) seguindo a receita oficial do vLLM para a RTX Pro 6000 e os model cards. Meça qualidade e velocidade sem usar nenhuma tarefa da avaliação, escolha pela regra da seção 2.0 e fixe o vencedor para a sessão inteira. Nada de fine-tunes, "uncensored" ou merges da comunidade.
4. Escreva WORKDIR/PLANO.md: a lista numerada de todos os passos do documento, fase por fase, até EXECUTAR_ATE, cada passo com o critério de "pronto" (o que precisa existir ou passar). Marque o que depende de decisão minha. Depois execute o plano na ordem, sem pular fases.
5. Mantenha WORKDIR/PROGRESSO.md atualizado a cada passo concluído: o que foi feito, comando principal, resultado (números reais), arquivos gerados, problemas. Mantenha também o WORKDIR/state.json que o documento pede, para retomar sem refazer nada se a sessão cair. Ao reiniciar, leia PROGRESSO.md e state.json primeiro e continue de onde parou.
6. Fim de cada fase: confira os critérios de "pronto" daquela fase e escreva um resumo curto em PROGRESSO.md (o que passou, o que falhou, números principais). Só então comece a próxima. As fases 0 e 1 bloqueiam: se algo falhar ali, resolva antes de seguir (decisão 3 do documento).
7. Escreva os scripts que o documento pede (proxy de medição, coletor de /metrics e nvidia-smi, orquestrador com rampa e retomada, cópia e isolamento dos exercícios, correção, anonimização para a revisão às cegas, análise estatística, relatório). Guarde todos em WORKDIR/tools/, com um README curto, e teste cada um isoladamente antes de usá-lo na execução que conta.
8. Tudo que for sintaxe de ferramenta de terceiros (flags do vLLM, OpenCode, Qwen Code, Claude Code, LiteLLM, vllm bench, Harbor, Pier) você confere com --help ou na documentação da versão instalada. Nunca use sintaxe de memória. Registre a versão de cada ferramenta.
9. Ordem: em toda comparação (jg-eng-tests e cada benchmark público) o fh roda primeiro, no modo 1 agente e depois no modo máximo, e só então os outros harnesses. Não troque essa ordem.
10. Não invente resultados. O que não foi medido fica NOT_RUN ou N/D, com o motivo. Um resultado ruim do nosso harness é um achado: registre com evidência e não esconda.

QUANDO PARAR E ME PERGUNTAR
- O HEAD do jg-eng-tests não é 36cbe4741e5730fae876fae3bff6fa167fb18cb7.
- Nenhum checkpoint candidato sobe no vLLM desta máquina, nem a configuração de referência do documento.
- Falta algo que só eu posso dar (credencial, token do Hugging Face para um checkpoint restrito, acesso, instalação que exige root que você não tem, espaço em disco).
- Falta acesso ou chave que só eu posso dar para um benchmark (conjunto privado do CWE-bench, chaves dos juízes do CWE-bench ou do avaliador do Vibe Code Bench).
- Uma regra do documento se mostrou impossível de cumprir como está escrita. Explique e proponha a alternativa; não improvise em silêncio.
Fora esses casos, siga sozinho até EXECUTAR_ATE.

O QUE NÃO FAZER
- Não altere o jg-eng-tests nem os graders.
- Não corrija o nosso harness (fh) no meio de uma execução que conta. Achados vão para o relatório. Correções só antes de começar, ou depois, como variante com nome próprio, conforme o documento.
- Não dê dicas aos agentes avaliados, nem mostre a eles grader, solution ou EVALUATION.md.
- Não use o Qwen/vLLM, nem nenhum harness avaliado, para julgar, resumir ou classificar resultados. O julgamento final é seu.
- Não mude a configuração do servidor entre harnesses na comparação principal.
- Não faça push para o repositório do harness. Tudo fica em WORKDIR.

ENTREGA
Ao terminar (ou ao atingir EXECUTAR_ATE ou um limite), gere os entregáveis de WORKDIR/relatorio/ descritos no documento e me responda com:
0. Resumo da revisão (REVISAO.md): o que estava errado no plano e o que você ajustou. O checkpoint e o comando do vLLM escolhidos, com os números de qualidade e velocidade que decidiram.
A. Ranking final (aprovação final com intervalo de Wilson), e ao lado o ranking só pelo grader.
B. Frankenstein Harness 1 agente contra modo máximo, com a regra de fan-out.
C. Benchmarks públicos: nota de cada harness em Terminal-Bench 4.0, DeepSWE v1.1, CWE-bench v1, Vibe Code Bench e FrontierSWE v2, com intervalo, e o que ficou NOT_RUN/UNSUPPORTED em cada um.
D. N_MAX escolhido na rampa e por quê. Se a fase 8 rodou, a capacidade em três níveis (confortável, aceitável, limite) com o tok/s médio por usuário e a configuração recomendada.
E. O que ficou NOT_RUN ou UNSUPPORTED, com o motivo.
F. Os três achados mais importantes sobre o nosso harness e os exercícios classificados como ambíguos ou defeituosos.
G. Caminhos dos arquivos (sem colar logs, a não ser que algo tenha falhado).
```

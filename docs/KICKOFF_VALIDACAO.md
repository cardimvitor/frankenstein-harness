# Prompt de partida: entregar a validação a outra LLM

Cole o bloco abaixo numa sessão de agente com terminal na máquina da GPU (por exemplo, Claude Code). Preencha os campos entre `< >` antes. Não cole chaves nem tokens no prompt: exporte-os no shell antes de abrir a sessão.

```text
Você vai conduzir, do começo ao fim, a validação de harnesses de código descrita num documento do meu repositório. Você é a LLM operadora e juíza: executa, mede e julga. O modelo avaliado (Frankenstein V2, um Qwen servido pelo meu vLLM) nunca julga nada.

DADOS
- Repositório do nosso harness: https://github.com/cardimvitor/frankenstein-harness, branch ccr-3bc51f62-prkdq0. Use as credenciais git que já existem nesta máquina; não me peça tokens.
- Documento a seguir: docs/VALIDACAO_JG_ENG_TESTS_V2.md nesse repositório. Ele é a especificação. Onde este prompt e o documento divergirem, vale o documento, e a seção "Decisões do dono do projeto" dele vale acima de tudo.
- GPU: <ex.: RTX PRO 6000 Blackwell 96 GB>
- WORKDIR: <ex.: /workspace/harness-eval>
- EXECUTAR_ATE: fase <0 a 8> (7 = benchmarks públicos; 8 = teste de capacidade do fh, sempre por último)
- Segredos: qualquer chave já está exportada no shell. Nunca a imprima, grave em arquivo, log, commit ou relatório.

COMO TRABALHAR
1. Clone o repositório em WORKDIR/frankenstein-harness, faça checkout da branch e leia docs/VALIDACAO_JG_ENG_TESTS_V2.md inteiro antes de executar qualquer coisa. Leia também README.md, docs/FANOUT.md, docs/LITELLM.md e integrations/harbor/README.md, porque o documento se refere a eles.
2. Escreva WORKDIR/PLANO.md: a lista numerada de todos os passos do documento, fase por fase, até EXECUTAR_ATE, cada passo com o critério de "pronto" (o que precisa existir ou passar). Marque o que depende de decisão minha. Depois execute o plano na ordem, sem pular fases.
3. Mantenha WORKDIR/PROGRESSO.md atualizado a cada passo concluído: o que foi feito, comando principal, resultado (números reais), arquivos gerados, problemas. Mantenha também o WORKDIR/state.json que o documento pede, para retomar sem refazer nada se a sessão cair. Ao reiniciar, leia PROGRESSO.md e state.json primeiro e continue de onde parou.
4. Fim de cada fase: confira os critérios de "pronto" daquela fase e escreva um resumo curto em PROGRESSO.md (o que passou, o que falhou, números principais). Só então comece a próxima. As fases 0 e 1 bloqueiam: se algo falhar ali, resolva antes de seguir (decisão 3 do documento).
5. Escreva os scripts que o documento pede (proxy de medição, coletor de /metrics e nvidia-smi, orquestrador com rampa e retomada, cópia e isolamento dos exercícios, correção, anonimização para a revisão às cegas, análise estatística, relatório). Guarde todos em WORKDIR/tools/, com um README curto, e teste cada um isoladamente antes de usá-lo na execução que conta.
6. Tudo que for sintaxe de ferramenta de terceiros (flags do vLLM, OpenCode, Qwen Code, Claude Code, LiteLLM, vllm bench, Harbor, Pier) você confere com --help ou na documentação da versão instalada. Nunca use sintaxe de memória. Registre a versão de cada ferramenta.
7. Ordem: em toda comparação (jg-eng-tests e cada benchmark público) o fh roda primeiro, no modo 1 agente e depois no modo máximo, e só então os outros harnesses. Não troque essa ordem.
8. Não invente resultados. O que não foi medido fica NOT_RUN ou N/D, com o motivo. Um resultado ruim do nosso harness é um achado: registre com evidência e não esconda.

QUANDO PARAR E ME PERGUNTAR
- O HEAD do jg-eng-tests não é 36cbe4741e5730fae876fae3bff6fa167fb18cb7.
- O vLLM não sobe com nenhuma das duas configurações do documento.
- Falta algo que só eu posso dar (credencial, acesso, instalação que exige root que você não tem, espaço em disco).
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
A. Ranking final (aprovação final com intervalo de Wilson), e ao lado o ranking só pelo grader.
B. Frankenstein Harness 1 agente contra modo máximo, com a regra de fan-out.
C. Benchmarks públicos: nota de cada harness em Terminal-Bench 4.0, DeepSWE v1.1, CWE-bench v1, Vibe Code Bench e FrontierSWE v2, com intervalo, e o que ficou NOT_RUN/UNSUPPORTED em cada um.
D. N_MAX escolhido na rampa e por quê. Se a fase 8 rodou, a capacidade em três níveis (confortável, aceitável, limite) com o tok/s médio por usuário e a configuração recomendada.
E. O que ficou NOT_RUN ou UNSUPPORTED, com o motivo.
F. Os três achados mais importantes sobre o nosso harness e os exercícios classificados como ambíguos ou defeituosos.
G. Caminhos dos arquivos (sem colar logs, a não ser que algo tenha falhado).
```

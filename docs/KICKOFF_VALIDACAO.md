# Prompt de partida: entregar a validação a outra LLM

Cole o bloco abaixo numa sessão de agente com terminal na máquina da GPU (por exemplo, Claude Code). Preencha os campos entre `< >` antes. Não cole chaves nem tokens no prompt: exporte-os no shell antes de abrir a sessão.

```text
Você vai revisar o meu plano de validação e depois conduzi-lo do começo ao fim. O plano compara oito configurações do mesmo modelo: o modelo sozinho (`modelo-direto`, a linha de base), três modos do nosso harness `fh` (`fh-nomem`, `fh-mem1`, `fh-memmax`) e os harnesses OpenCode, Qwen Code, Claude Code e DeepSeek. Você é a LLM operadora e juíza: revisa, configura, executa, mede e julga. O modelo avaliado (Frankenstein V2, um Qwen3.8-27B servido pelo meu vLLM nesta máquina) nunca julga nada.

DADOS
- Repositório do nosso harness: https://github.com/cardimvitor/frankenstein-harness, branch ccr-3bc51f62-prkdq0. Use as credenciais git que já existem nesta máquina; não me peça tokens.
- Documento a seguir: docs/VALIDACAO_JG_ENG_TESTS_V2.md nesse repositório. Ele é a especificação. Onde este prompt e o documento divergirem, vale o documento, e a seção "Decisões do dono do projeto" dele vale acima de tudo.
- GPU: uma NVIDIA RTX PRO 6000 Blackwell, 96 GB (SM120). É a única GPU: ela serve o modelo avaliado e mais nada.
- WORKDIR: <ex.: /workspace/harness-eval>
- EXECUTAR_ATE: fase <0 a 8> (7 = benchmarks públicos DeepSWE v1.1 e FrontierSWE v2; 8 = teste de capacidade do fh, sempre por último)
- Contêineres: use Docker; se a máquina não tiver, use Podman ou equivalente (seção 1, item 9 do documento) e confirme que o Harbor e o Pier funcionam com ele.
- Segredos: qualquer chave já está exportada no shell. Nunca a imprima, grave em arquivo, log, commit ou relatório.

COMO TRABALHAR
1. Clone o repositório em WORKDIR/frankenstein-harness, faça checkout da branch e leia docs/VALIDACAO_JG_ENG_TESTS_V2.md inteiro antes de executar qualquer coisa. Leia também README.md, docs/FANOUT.md, docs/LITELLM.md e integrations/harbor/README.md, porque o documento se refere a eles.
2. Prepare a máquina e dispare os downloads (itens 7 e 8 da fase 0 do documento, decisão 9). Faça logo depois de ler o plano, e deixe os downloads rodando **em paralelo** com a revisão do passo 3:
   - A máquina e a GPU são inteiramente da avaliação. Libere toda a VRAM: pare e mate o que usar a placa (vLLM ou Ollama antigos, notebooks, treinos, contêineres com --gpus) e os processos que gastem CPU e RAM sem fazer parte da avaliação. Primeiro SIGTERM, depois SIGKILL; impeça que voltem; confirme no nvidia-smi que não há processo de computação e que a VRAM usada está abaixo de 500 MiB.
   - Antes, grave o inventário (WORKDIR/maquina/antes.txt); depois registre o que parou e como restaurar (WORKDIR/maquina/parado.md).
   - Nunca mate: a sua própria sessão e shell, sshd e conexões SSH ativas, systemd, dockerd e containerd, a rede, o driver da NVIDIA. Pergunte antes de parar processo de outro usuário com sessão ativa, ou serviço que pareça de produção ou guarde dados de outras pessoas.
   - Aplique os ajustes de desempenho da fase 0 (modo persistente da GPU, governador de CPU, ulimit, /dev/shm) e deixe 4 núcleos e 16 GB de RAM livres para o vLLM e o proxy.
   - Detecte o runtime de contêineres (Docker, senão Podman) e prove que funciona com o Harbor e o Pier (item 9 da fase 0).
   - Confira o espaço em disco e baixe e instale tudo em paralelo, em segundo plano, com log por item em WORKDIR/instalacao/: checkpoints, vLLM, imagens Docker, toolchains, harnesses, Harbor e Pier, datasets e repositórios dos benchmarks, caches de pacotes dos exercícios. No máximo 4 a 6 downloads ao mesmo tempo. Compile o fh enquanto baixa. Ao fim, verifique checksums e versões (WORKDIR/instalacao/VERSOES.md).
3. Revise o plano antes de executar. Escreva WORKDIR/REVISAO.md com:
   - tudo que estiver errado, ambíguo, contraditório, desatualizado ou impossível nesta máquina, com a seção do documento e a correção proposta;
   - o que falta para cumprir as decisões do dono;
   - os riscos de tempo, disco e GPU.

   Confira de verdade, não só lendo: versões e nomes de datasets (Harbor Hub, repositórios dos benchmarks), existência do checkpoint e da revisão `dbb8f445` no Hugging Face, flags do vLLM instalado, e se os scripts e adaptadores do repositório rodam (`cargo test --release`, `python3 -m py_compile integrations/harbor/fh_harbor/*.py`). Corrija o que for claramente erro de digitação ou de comando na sua cópia de trabalho do plano (WORKDIR/plano-ajustado.md) e anote cada mudança. Mudanças que alteram uma decisão minha, ou que mudam o que é comparado, você me pergunta antes. Depois siga.
4. Com a GPU livre e o download pronto, suba o modelo fixo (decisão 8 e seção 2.0 do documento): `nvidia/Qwen3.8-27B-NVFP4` (https://huggingface.co/nvidia/Qwen3.8-27B-NVFP4), revisão `dbb8f445`, com MTP 3. É o modelo que eu já validei: não compare outros checkpoints, não varra o MTP, não baixe BF16, FP8 nem quantizações da comunidade. Use o comando da seção 2.1, confira cada flag com `vllm serve --help`, ajuste só o que a máquina exigir (memória da GPU, contexto, `max-num-seqs`) e registre. Depois rode a verificação da seção 2.0: com prompts de agente, a geração precisa ficar perto de 122,1 tok/s por fluxo e 75,7% de aceitação do MTP (até 15% de diferença). Se ficar fora, corrija o ambiente, não o modelo; se não resolver, me pergunte. Fixe o comando para a sessão inteira.
5. Escreva WORKDIR/PLANO.md: a lista numerada de todos os passos do documento, fase por fase, até EXECUTAR_ATE, cada passo com o critério de "pronto" (o que precisa existir ou passar). Marque o que depende de decisão minha. Depois execute o plano na ordem, sem pular fases.
6. Mantenha WORKDIR/PROGRESSO.md atualizado a cada passo concluído: o que foi feito, comando principal, resultado (números reais), arquivos gerados, problemas. Mantenha também o WORKDIR/state.json que o documento pede, para retomar sem refazer nada se a sessão cair. Ao reiniciar, leia PROGRESSO.md e state.json primeiro e continue de onde parou.
7. Fim de cada fase: confira os critérios de "pronto" daquela fase e escreva um resumo curto em PROGRESSO.md (o que passou, o que falhou, números principais). Só então comece a próxima. As fases 0 e 1 bloqueiam: se algo falhar ali, resolva antes de seguir (decisão 3 do documento).
8. Escreva os scripts que o documento pede (proxy de medição, coletor de /metrics e nvidia-smi, orquestrador com rampa e retomada, cópia e isolamento dos exercícios, correção, anonimização para a revisão às cegas, análise estatística, relatório). Guarde todos em WORKDIR/tools/, com um README curto, e teste cada um isoladamente antes de usá-lo na execução que conta.
9. Tudo que for sintaxe de ferramenta de terceiros (flags do vLLM, OpenCode, Qwen Code, Claude Code, LiteLLM, vllm bench, Harbor, Pier) você confere com --help ou na documentação da versão instalada. Nunca use sintaxe de memória. Registre a versão de cada ferramenta.
10. Todas as configurações rodam em paralelo, isoladas (contêiner, rede, chave, cotas, diretórios e memória próprios; seção 5.0 do documento), com um orçamento global de tarefas simultâneas medido na calibração: começa em 5 e sobe de 5 em 5 (5, 10, 15, 20, ...), e para quando o tok/s total do servidor cresce menos de 5% ou cai, ou o tok/s por tarefa fica abaixo de 30; o último degrau bom vira o N_MAX, e a execução principal já começa nele, validado pela checagem de mistura (seção 5.0a). **Nenhum teste é pulado** (decisão 14 do documento): todos os exercícios, todas as tarefas dos dois benchmarks (inclusive as 11 do FrontierSWE que pedem GPU, no bloco de GPU compartilhada da seção 8.3), todas as técnicas e variantes da fase 8. Tempo não é motivo para cortar: aumente o paralelismo dentro das guardas, retome pelo state.json e informe o dono do tempo restante a cada fase. Só o dono decide cortar algo. O fh vem na frente: as três configurações dele entram primeiro na fila e aparecem primeiro nos relatórios. Como elas dividem a GPU, a velocidade comparável vem do passe de tempo (seção 5.0c), com cada configuração sozinha.
11. Antes de rodar o fh nos modos do plano, confirme que o binário tem os modos novos (`fh --help` lista `--no-memory`, `--consolidate` e `fh direct`) e rode `cargo test --release`. Não implemente nem altere modos durante a execução que conta.
12. Não invente resultados. Nada é pulado: o que for fisicamente inviável neste hardware é executado mesmo assim, o log da falha é a evidência, e você avisa o dono sem parar. Antes de declarar um harness UNSUPPORTED, tente plugá-lo (adaptador próprio) e avise o dono. Um resultado ruim do nosso harness é um achado: registre com evidência e não esconda.

QUANDO PARAR E ME PERGUNTAR
- O HEAD do jg-eng-tests não é 36cbe4741e5730fae876fae3bff6fa167fb18cb7.
- O modelo `nvidia/Qwen3.8-27B-NVFP4` na revisão `dbb8f445` não baixa ou não sobe no vLLM desta máquina com MTP 3, ou a velocidade e a aceitação do MTP ficam fora da referência (122,1 tok/s por fluxo, 75,7%) mesmo depois de corrigir o ambiente.
- Falta algo que só eu posso dar (credencial, token do Hugging Face para um checkpoint restrito, acesso, instalação que exige root que você não tem, espaço em disco).
- Docker e Podman faltam e você não consegue instalar nenhum dos dois.
- Um processo que você ia parar é de outro usuário com sessão ativa ou parece um serviço de produção, ou a VRAM não libera nem com o reset da GPU (pode exigir reiniciar a máquina).
- Uma regra do documento se mostrou impossível de cumprir como está escrita. Explique e proponha a alternativa; não improvise em silêncio.
Fora esses casos, siga sozinho até EXECUTAR_ATE.

O QUE NÃO FAZER
- Não altere o jg-eng-tests nem os graders.
- Não corrija o nosso harness (fh) no meio de uma execução que conta. Achados vão para o relatório. Correções só antes de começar, ou depois, como variante com nome próprio, conforme o documento.
- Não dê dicas aos agentes avaliados, nem mostre a eles grader, solution ou EVALUATION.md.
- Não use o Qwen/vLLM, nem nenhum harness avaliado, para julgar, resumir ou classificar resultados. O julgamento final é seu.
- Não mude a configuração do servidor entre harnesses na comparação principal.
- Não faça push para o repositório do harness. Tudo fica em WORKDIR.
- Ao liberar recursos, não mate a sua própria sessão, o sshd, a rede, o dockerd nem o driver da GPU, e não apague volumes, bancos de dados nem arquivos de pessoas.

ENTREGA
Ao terminar (ou ao atingir EXECUTAR_ATE ou um limite), gere os entregáveis de WORKDIR/relatorio/ descritos no documento e me responda com:
0. Resumo da revisão (REVISAO.md): o que estava errado no plano e o que você ajustou. O comando exato do vLLM (modelo fixo, MTP 3), os ajustes que a máquina exigiu e os números de verificação (tok/s por fluxo e aceitação do MTP) contra a referência.
A. Ranking das oito configurações (aprovação final com intervalo de Wilson), e ao lado o ranking só pelo grader. Para cada uma, se melhorou ou piorou o modelo sozinho.
B. A cadeia de ablação: `modelo-direto` → `fh-nomem` (o harness) → `fh-mem1` (a memória) → `fh-memmax` (a paralelização com consolidação), com a diferença de taxa de aprovação, o intervalo e o custo em tokens de cada passo, e a regra de fan-out aplicada ao último.
C. Benchmarks públicos: nota de cada configuração em DeepSWE v1.1 e FrontierSWE v2, com intervalo, a diferença de cada uma para o `modelo-direto`, e o que ficou inviável neste hardware ou UNSUPPORTED em cada um, com a evidência.
D. N_MAX escolhido na rampa e por quê. Se a fase 8 rodou, a capacidade em três níveis (confortável, aceitável, limite) com o tok/s médio por usuário e a configuração recomendada.
E. O que ficou inviável neste hardware, UNSUPPORTED ou PENDENTE, com a evidência de cada tentativa.
F. Os três achados mais importantes sobre o nosso harness e os exercícios classificados como ambíguos ou defeituosos.
G. Caminhos dos arquivos (sem colar logs, a não ser que algo tenha falhado).
```

# 선택적 LLM 정책

기본 빌드는 HTTP 의존성 없이 실행합니다. `llm` Cargo feature를 켠 앱에서만 Tokio와 Reqwest 어댑터를 사용할 수 있습니다. 엔진의 권한은 여전히 단일 스레드에 있으며, 공급자는 읽기용 관찰을 받고 선언적인 Action만 제안합니다.

## 실행

```sh
cargo run -p agentsociety-cli --features llm -- --agents 100 --ticks 100 --policy llm --llm-agents 4 --llm-config examples/llm-ollama.json --events runs/llm-events.jsonl --state-out runs/llm-state.json
cargo run -p agentsociety-desktop --features llm -- --agents 100 --policy hybrid --llm-agents 4 --llm-config examples/llm-ollama.json
```

Ollama 서버와 예시 JSON의 모델은 사용자가 준비해야 합니다. 모델 이름은 로컬에서 제공 중인 이름으로 변경할 수 있습니다. 서버가 없어도 연결 실패를 기록하고 규칙 정책으로 계속 진행합니다. 최초 모델 로딩이 배치 제한 시간보다 길면 제한 시간을 조정하거나 모델을 미리 로드할 수 있습니다.

OpenAI 호환 예시는 `examples/llm-openai-compatible.json`입니다. 기본 예시는 OpenAI 주소와 `gpt-4o-mini`이며, 다른 호환 서버의 기본 주소·모델로 바꿀 수 있습니다. 필요한 키는 `api_key_env`가 지정하는 환경변수에서 읽습니다. 키가 필요 없는 로컬 서버는 이 값을 `null`로 둡니다. `.env`는 자동 로드하지 않습니다.

`--policy`의 값은 `rule-based`, `llm`, `hybrid`입니다. `SimulationConfig` JSON에서는 `rule_based`, `llm`, `hybrid`를 사용합니다. 처음 `llm_agents`명의 에이전트에 지정 정책을 적용하고 나머지는 규칙 정책을 사용합니다. 외부 정책에 필요한 feature나 설정이 없으면 시작 시 명확한 오류를 냅니다.

## 정책과 실행 경계

- `RuleBasedPolicy`: 기존의 목표·욕구·재고·가격 기반 정책입니다.
- `LlmPolicy`: 최소 호출 간격과 예산 안에서 모델에 행동을 요청합니다. 건너뛰거나 실패한 요청은 규칙 정책을 사용합니다.
- `HybridPolicy`: 생존에 필요한 일상 행동은 규칙으로 처리합니다. 최근 네 행동이 같거나 최근 네 번 중 두 번 이상 거절됐을 때 전략 재검토를 요청합니다. 에너지 부족·기아·식량 비축 부족이면 모델을 기다리지 않고 규칙을 사용합니다.

완료 Tick T의 상태를 읽어 T+1용 요청을 만듭니다. 응답을 기다리는 동안 월드를 변경하지 않습니다. 결과는 `BTreeMap<AgentId, ExternalDecision>`으로 모아 AgentId 순서로 기록합니다. 비동기 완료 순서에 따라 정산 순서가 달라지지 않습니다.

배치 전체에 하나의 마감 시간을 적용합니다. 동시성 슬롯을 기다리는 요청도 같은 마감 시간에 취소되며, 늦은 결과를 이후 Tick에 적용하지 않습니다. GUI는 별도 worker에서 이 과정을 실행하므로 화면 읽기와 렌더링이 HTTP에 잠기지 않습니다. Pause/Restart는 현재 배치와 Tick이 끝나는 경계에서 처리됩니다.

## 한도와 응답 검증

| 설정 | 기본값 | 의미 |
| --- | ---: | --- |
| `timeout_ms` | 1,500 | 전체 결정 배치 제한 시간, 최대 30,000ms |
| `max_concurrency` | 2 | 동시에 수행할 HTTP 요청 수 |
| `max_calls_per_tick` | 4 | 한 Tick에서 예약할 요청 수 |
| `max_total_calls` | 100 | 실행 전체 예약 요청 수, 0이면 요청하지 않음 |
| `min_interval_ticks` | 10 | 실패를 포함한 같은 에이전트의 요청 간격 |
| `max_output_tokens` | 256 | 공급자에 전달하는 생성 토큰 제한 |
| `memory_limit` | 8 | 검색한 기억 수, 최대 16 |

아직 전송되지 않고 동시성 슬롯을 기다리다 취소된 요청도 보수적으로 호출 예산에 포함합니다. 자동 HTTP 재시도와 리다이렉트는 사용하지 않습니다. 사용자가 `Restart`하면 요청 예산과 호출 간격 기록도 새 실행에 맞춰 초기화됩니다.

관찰에는 본인 상태와 목표 성과 구간, 시장 가격·수요·공급, 최근 결정 최대 4개, 관련 기억 최대 16개, 주요 관계 최대 4개를 포함합니다. 협상·협력이 켜져 있으면 본인의 응답 가능한 제안 최대 4개와 상대의 공개 요약 최대 8개도 제공합니다. 전체 에이전트의 기억이나 월드를 보내지 않습니다. 관찰 JSON은 최대 32KiB, HTTP 응답은 최대 64KiB, 짧은 설명은 최대 512자입니다.

모델의 응답 형식은 다음과 같습니다.

```json
{
  "action": {"action": "buy", "resource": "food", "quantity": 2, "limit_price": 110},
  "reasoning_summary": "식량 비축량을 보충한다."
}
```

마크다운 코드 블록, 알 수 없는 필드·행동, 잘못된 타입, 음수 수량, 긴 설명을 거절합니다. 기본 행동은 `produce/gather/consume/buy/sell/move/rest/wait`이며 활성화된 기능에 한해 `negotiate/counter_offer/accept_proposal/reject_proposal/cancel_proposal/cooperate`도 검증합니다. 구조 검증 다음에는 관찰한 월드의 현금·재고·직업·에너지·좌표와 제안의 역할·기한·한도를 확인합니다. 실제 적용 시 엔진이 다시 검증하므로 여러 에이전트의 행동이 공유 자원에 충돌해도 검사를 우회하지 않습니다. 이 마지막 검사에서 거절될 수 있으므로 외부 결과의 `accepted`는 거래 체결이나 모든 행동의 실행 성공을 뜻하지 않습니다. 새 행동 JSON과 경제적 의미는 [상호작용 문서](interactions.md)에 있습니다.

실패 사유는 `invalid_response`, `invalid_action`, `timeout`, `provider_error`, `budget_exhausted`로 기록합니다. 원본 HTTP 오류 본문, Authorization 헤더, 키는 저장하지 않습니다. 공급자가 JSON 모드를 지원하지 않는 등 프로토콜이 맞지 않아도 규칙 대체로 처리합니다.

## 기록과 재적용

`ExternalDecisionRecorded`에는 대상 Tick, AgentId, 프로토콜·모델, 결과, 승인된 Decision, 짧은 설명이 있습니다. 그 뒤 `DecisionRecorded`에 실제 선택된 외부 또는 규칙 의도가 남습니다. Inspector는 마지막 모델 결과와 대체 사유를 표시합니다.

`Simulation::step_with_decision_batch`로 같은 배치를 다시 적용하면 모델을 호출하지 않고 같은 이벤트·경제 상태·기억을 재현할 수 있습니다. 이 API는 `llm` feature가 없는 빌드에도 존재합니다. 오래됐거나 미래인 대상 Tick, 다른 AgentId, 결과와 Decision의 모순은 월드를 바꾸기 전에 거절합니다.

CLI `replay <JSONL 또는 SQLite>`는 기록한 외부 결과와 실제 결정을 사용하고 모든 재생성 이벤트·최종 상태를 비교합니다. HTTP feature, 서버, API 키가 필요하지 않습니다. 체크포인트와 SQLite에서 `resume`할 때는 공급자 설정, 누적 예산, 마지막 요청 Tick을 복원합니다. 따라서 이어 실행이 요청 예산을 다시 채우거나 호출 간격을 건너뛰지 않습니다. 새로운 외부 호출을 위해서는 `llm` feature와 해당 환경변수 자격증명이 필요합니다. 키 값은 저장하지 않고 `api_key_env` 이름만 저장합니다.

`--state-out`만으로는 외부 정책의 호출 이력을 복원할 수 없으므로 `--checkpoint-out` 또는 `--database`를 사용합니다. `resume --llm-config`를 지정한다면 저장한 설정과 정확히 같아야 합니다. 복원 후 공급자나 예산을 변경하는 분기 실행은 아직 지원하지 않습니다. [저장·복원](persistence.md)에 명령 예시와 손상 처리 범위를 설명합니다.

CLI 요약의 `llm_usage`는 예약 요청·승인·대체 수와 공급자가 보고한 입력/출력 토큰 합계를 제공합니다. 잘못된 모델 JSON도 사용량이 보고됐다면 토큰 합계에 포함합니다. 보고되지 않은 사용량을 추정하지 않습니다. 금액 환산은 구현하지 않았습니다.

## 프로토콜 근거와 검증 범위

Ollama는 [`POST /api/chat`](https://docs.ollama.com/api/chat)의 `stream: false`, JSON 출력, `message.content`를 사용합니다. OpenAI 호환 어댑터는 [Chat Completions 공식 API](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)의 메시지 배열, JSON 출력 모드, `max_completion_tokens`, `choices[0].message.content`를 사용합니다. JSON 모드는 스키마를 보장하지 않으므로 Rust에서 별도로 엄격히 검증합니다. 공급자와 모델의 선택 파라미터 지원 차이는 오류 및 대체 사유로 드러납니다.

자동 테스트는 실제 loopback TCP/HTTP를 사용하는 Rust 서버로 수행합니다. 두 공급자 형식, 환경변수 인증, 잘못된 JSON·불가능한 행동·큰 응답·불완전한 생성, 동시성·예산·타임아웃·리다이렉트 차단, 서버 중단 시 진행, UI 읽기, 결정 재적용을 확인합니다. 실제 유료 API 호출, 실제 모델의 판단 품질, Ollama 모델 설치 상태를 검증한 것은 아닙니다.

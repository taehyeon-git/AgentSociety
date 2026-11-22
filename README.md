# AgentSociety

Rust 기반 자율 에이전트 사회·경제 시뮬레이터입니다. **CLI·GUI, 기억·LLM, 저장·복원·Replay, 협상·목표 적응, 다자 조직·공동 재산·장기 계약, 공급 충격·재정 정책과 비교 실험**을 구현했습니다. 각 에이전트는 자신의 목표·욕구·재고·현금에 따라 행동하고, 실제 거래와 생산 결과가 자산과 가격을 바꿉니다. 기본 모드에는 Python이나 LLM, 실행 중 네트워크 연결이 필요하지 않습니다.

## 구현된 기능

- Farmer, Producer, Merchant, Consumer와 네 종류의 목표, 개별 성격·기술·욕구·위치·최근 행동 이력
- `bevy_ecs` 월드, 고정 시드 ChaCha8 난수, 단일 스레드 Tick 실행, 읽기 전용 의사결정 정책
- Food / Wood / Metal / Goods 생산·채집·제조·소비와 현금/재고를 예약하는 중앙 주문장
- 가격·시간 우선 매칭, 부분 체결, 자기 거래 방지, 미체결 만료, 원자적 양자 정산
- 매 Tick 개인·공동·정부 계좌의 화폐 보존, 자원 수지, 양자 제안·장기 계약 담보와 이체 이력 검사
- 거래량·가격·평균 자산·Gini·생산/소비·기아 통계와 실행 시간 측정
- 선택적 JSONL 이벤트, Tick별 CSV, 요약 JSON, 결정론적 최종 상태 JSON 출력
- eframe 대시보드: 지도·에이전트 선택·Inspector·가격/Gini 그래프·이벤트 필터·실행/일시정지/Step/재시작/속도 제어
- 실제 거래에 따른 신뢰·친밀도·상호작용 기록, 동일 가격에서 신뢰하는 상대 선호, petgraph 관계 분석
- 에이전트별 Episodic/Social/Economic/Goal 기억, 최근성·중요도·관련도 검색, 보관 수와 문맥 크기 제한
- 선택적 Ollama/OpenAI 호환 HTTP 정책, Llm/Hybrid와 규칙 에이전트 혼합, 호출 예산·동시성·배치 타임아웃·실패 시 규칙 대체
- 외부 결정·대체 사유 기록과 네트워크 없는 결정 배치 재적용 API
- 버전·불변조건을 검사하는 체크포인트, LLM 예산·호출 간격을 유지하는 `resume`
- JSONL/SQLite의 모든 이벤트와 최종 상태를 검증하는 오프라인 `replay`
- SQLite Tick 단위 트랜잭션, 주기적 체크포인트, 중단 후 복구와 에이전트 이력 조회
- 시나리오·시드·인구·정책 조합별 실행과 자산 분포·사회관계·LLM 사용량 비교
- 상대를 지정한 가격 협상·역제안·수락·거절·철회·만료, 공동 제조와 보상 배분
- 실제 기아·거래·협력·기준 자산에 따른 주기적 목표 변경과 변경 이력
- 다자 조직 설립·가입·탈퇴, 공동 계좌·3인 제조·동일 배당, 분할 납품과 예약 대금·이행 보증금
- 시간 지정 공급 충격·재고 손실, 정부 금고를 통한 판매세·현금 부담금·재분배
- 현재 상태와 난수·LLM 예산을 유지하고 미래 일정만 바꾸는 `branch`
- GUI 폴더·파일 선택, 체크포인트 저장, JSON/SQLite/JSONL 불러오기, 실패 시 현재 실행 보존

새 기능은 기본값에서 꺼져 있습니다. `examples/complete.json`으로 협상·목표 적응·조직·계약·위기·재정 정책을 함께 실행할 수 있습니다.

## 준비와 실행

Rust Stable **1.95 이상**이 필요합니다. 이 작업 환경에서는 Rust/Cargo 1.98.1로 검증했습니다. 의존성 버전은 `Cargo.lock`으로 고정합니다. SQLite 번들 소스를 위해 C 컴파일러도 필요합니다. Windows MSVC 타깃은 C++ Build Tools와 Windows SDK, Linux/macOS는 해당 네이티브 빌드 도구를 사용합니다.

```sh
cargo run -p agentsociety-cli -- --agents 100 --ticks 1000 --seed 42
cargo run -p agentsociety-cli -- --help
cargo run -p agentsociety-cli -- --config examples/tiny.json --ticks 100
cargo run -p agentsociety-desktop -- --agents 100 --seed 42
```

GUI는 일시정지 상태로 시작합니다. `Run`, `Pause`, `Step`, `Restart`로 진행을 제어하고 지도 마커를 클릭하여 에이전트를 고릅니다. `Social network`는 실제 관계를 표시하며, Inspector에서 최근 결정·기억·관계를 펼칠 수 있습니다. `--auto-start`는 실행 상태로 시작합니다. UI와 시뮬레이션 worker는 별도 스레드이며, 화면에는 완료된 Tick만 표시합니다. 화면에는 최근 600개 통계와 1,500개 이벤트를 보관합니다.

`Save checkpoint...`와 `Load run...`에서 폴더를 탐색해 저장·복원합니다. `Restart scenario...`는 다음 초기화의 스케줄을 선택하고, `Branch scenario...`는 현재 상태에서 미래 일정만 교체합니다. 공동 재산·계약과 세금·충격은 Inspector와 Economy에서 확인합니다. 자세한 사용법은 [GUI](docs/desktop.md)에 있습니다.

협상·공동 생산·목표 적응은 기본적으로 꺼져 있습니다. 함께 실행하려면 제공한 설정을 사용합니다. GUI에서도 `Social behavior` 설정을 바꾼 뒤 `Restart`로 적용할 수 있습니다.

```sh
cargo run --release -p agentsociety-cli -- --config examples/phase5.json --ticks 1000 --summary-out runs/phase5-summary.json
cargo run --release -p agentsociety-desktop -- --config examples/phase5.json
cargo run --release -p agentsociety-cli -- --config examples/complete.json --ticks 1000 --summary-out runs/complete-summary.json
cargo run --release -p agentsociety-desktop -- --config examples/complete.json
```

제안의 예약·만료, 공동 생산 비용과 몫, 목표 변경 조건은 [협상·협력·목표 적응](docs/interactions.md)에 설명합니다.

다자 공동 계좌와 분할 납품은 [조직·장기 계약](docs/organizations.md), 공급·재고 충격과 세금·재분배는 [시나리오](docs/scenarios.md)에 설명합니다.

## 선택적 LLM 모드

기본 빌드에는 HTTP 어댑터를 포함하지 않습니다. LLM을 사용하려면 `--features llm`, 정책, 공급자 설정을 함께 지정합니다. `llm_agents`에 해당하는 앞쪽 ID만 Llm/Hybrid 정책을 사용하고 나머지는 규칙 정책을 유지합니다.

```sh
cargo run -p agentsociety-cli --features llm -- --agents 100 --ticks 100 --policy llm --llm-agents 4 --llm-config examples/llm-ollama.json
cargo run -p agentsociety-desktop --features llm -- --agents 100 --policy hybrid --llm-agents 4 --llm-config examples/llm-ollama.json
```

Ollama 예시의 `model`은 로컬에 설치되어 실행 가능한 모델 이름으로 맞춥니다. OpenAI 호환 서버는 `examples/llm-openai-compatible.json`의 `base_url`, `model`, `api_key_env`를 설정합니다. 키는 지정한 환경변수에서만 읽으며 `.env` 파일을 자동으로 로드하지 않습니다. 설정 JSON에 실제 키를 넣지 않습니다.

기본 한도는 실행 전체 100회, Tick당 4회, 동시 요청 2개, 동일 에이전트 요청 간격 10 Tick, 배치 제한 시간 1,500ms입니다. 잘못된 응답, 불가능한 행동, 서버 중단, 예산 소진 시 규칙 정책으로 진행합니다. 외부 결과와 짧은 설명은 이벤트 및 Inspector에 남습니다. 상세 계약과 테스트 범위는 [LLM 연동](docs/llm.md)에 있습니다.

대규모 실행과 파일 출력:

```sh
cargo run --release -p agentsociety-cli -- --agents 1000 --ticks 1000 --seed 42 --report-every 0 --summary-out runs/1000-summary.json
cargo run --release -p agentsociety-cli -- --agents 100 --ticks 1000 --seed 42 --events runs/events.jsonl --state-out runs/final-state.json --summary-out runs/summary.json --csv runs/metrics.csv
cargo run -p agentsociety-cli -- --agents 0 --ticks 0 --json
```

기존 출력 파일을 덮어쓰려면 `--overwrite`를 명시합니다. `--json`은 stdout에 요약 JSON만 출력합니다. 진행 상황과 tracing 로그는 stderr로 보냅니다. `--report-every 0`은 진행 출력을 끕니다. `--config`의 값보다 `--agents`, `--seed`가 우선하며 알 수 없는 JSON 설정 키는 오류입니다.

빌드 의존성을 최초 확보한 후에는 다음과 같이 Cargo의 네트워크 접근도 차단한 채 실행할 수 있습니다.

```sh
cargo run --offline --locked -p agentsociety-cli -- --agents 100 --ticks 1000 --seed 42
```

현재 Windows 작업 폴더에는 시스템 설치를 바꾸지 않는 로컬 빌드 설정과 의존성 캐시가 준비되어 있습니다. Rust에 포함된 `rust-lld`, Microsoft 공식 CRT·C 컴파일러·언어 리소스, 기존 Windows SDK를 사용하며 `.cargo/config.toml`과 `.build-cache/`는 Git에서 제외합니다. 이 설정을 다시 생성하려면 다음 PowerShell 스크립트를 사용할 수 있습니다. 정상적인 MSVC 개발 환경에서는 필요하지 않습니다.

```powershell
.\scripts\setup-local-msvc.ps1 -SkipVendor
```

## 저장·복원·Replay

`--ticks`는 새 실행과 이어 실행 모두 **이번에 추가로 진행할 Tick 수**입니다. 체크포인트는 기본 100 Tick마다, 그리고 정상 종료 시 기록합니다.

```sh
cargo run --release -p agentsociety-cli -- run --config examples/phase5.json --ticks 500 --checkpoint-out runs/save/checkpoint.json --events runs/save/first.jsonl --database runs/save/run.sqlite
cargo run --release -p agentsociety-cli -- resume runs/save/checkpoint.json --ticks 500 --state-out runs/save/resumed.json
cargo run --release -p agentsociety-cli -- resume runs/save/run.sqlite --ticks 500
cargo run --release -p agentsociety-cli -- replay runs/save/run.sqlite --state-out runs/save/replayed.json --json
cargo run --release -p agentsociety-cli -- replay runs/save/first.jsonl --json
cargo run --release -p agentsociety-cli -- inspect runs/save/run.sqlite --agent 0 --from-tick 400 --to-tick 500 --limit 100 --json
```

규칙 전용 `--state-out` 파일도 복원할 수 있습니다. LLM 실행을 이어가려면 예산·호출 간격이 포함된 체크포인트나 SQLite가 필요합니다. 외부 모델이 포함된 기록도 `replay`에서는 HTTP feature나 API 키 없이 검증합니다. 출력은 기본적으로 기존 파일을 보호하며 `--overwrite`는 명시한 파일에만 적용합니다. 기존 SQLite는 덮어쓰지 않고 `resume`으로 이어갑니다.

현재 엔진은 **0.6.0**, 상태/이벤트 포맷은 **6**입니다. v0.5 등 이전 출력의 복원·마이그레이션은 지원하지 않습니다. 이전 CLI 실행 파일은 `.build-cache/releases/0.5.0/agentsociety-cli.exe`에 보관했습니다. JSONL은 완료 footer가 있어야 검증할 수 있고, 비정상 종료 후의 확정 Tick 복구에는 SQLite를 사용합니다. 형식과 실패 처리 계약은 [저장·복원](docs/persistence.md)에 있습니다.

## 여러 조건의 실험 비교

```sh
cargo run --release -p agentsociety-cli -- experiment --config examples/phase5.json --populations 10,100 --seeds 42,43,44 --policies rule-based --ticks 1000 --out-dir runs/experiments/social
cargo run --release -p agentsociety-cli -- experiment --config examples/complete.json --scenarios examples/scenarios/crisis.json,examples/scenarios/redistribution.json --include-baseline --seeds 42,43,44 --populations 100 --ticks 1000 --out-dir runs/experiments/scenarios
```

`report.json`과 `results.csv`에 실행별 결과와 시나리오·인구·정책별 비교를 저장하며, 각 실행의 설정·체크포인트·요약도 남깁니다. `--record-events`를 추가하면 규칙 실행도 JSONL을 기록합니다. 외부 정책 실행은 항상 기록합니다. 새 실험마다 LLM 예산이 별도로 적용되므로 전체 호출 한도는 실행 수만큼 증가할 수 있습니다. 지표 정의와 LLM 실험 방법은 [실험 비교](docs/experiments.md)에 있습니다.

같은 완료 상태에서 정책만 비교하려면 `branch`를 사용합니다. 새 일정은 저장 Tick 뒤에 시작해야 하고 원본 파일은 유지됩니다.

```sh
cargo run --release -p agentsociety-cli -- run --config examples/complete.json --ticks 500 --checkpoint-out runs/complete/half.json
cargo run --release -p agentsociety-cli -- branch runs/complete/half.json --scenario examples/scenarios/branch-after-500.json --ticks 500 --checkpoint-out runs/complete/branch.json --events runs/complete/branch.jsonl
```

## 결과 해석

화폐와 가격은 **정수 최소 단위**입니다. 100 최소 단위가 1 credit이며, 초기 가격 배열은 Food, Wood, Metal, Goods 순서입니다. 모든 자원 배열도 같은 순서를 사용합니다. 개인 자산은 현금·보유 자원·공동 지분을 현재 거래 가격으로 평가합니다. 정부 금고는 총 화폐에 포함하고 개인 Gini·평균 자산에서 제외합니다. 가격 변화로 평가 자산은 바뀌어도 화폐 공급은 그대로입니다.

Consumer는 원자재 채집·판매로 구매 자금을 마련합니다. 누구나 시장에서 식량을 살 수 없을 때 낮은 생산성의 자급 농업을 할 수 있습니다. 금속 재생은 지역의 새로운 채굴 용량에 접근하는 단순화이며 지질학적 재생을 뜻하지 않습니다. 재분배는 기존 금고 재원을 사용하며 통화 발행·신용·사망·직업 변경은 모델에 없습니다. 경제가 항상 균형에 도달하거나 모든 에이전트가 부유해진다는 가정도 없습니다.

같은 설정·시드·외부 결정 입력과 같은 규칙/의존성 버전을 사용하면 최종 상태와 이벤트 순서가 일치합니다. 벽시계 성능 수치는 결정론적 상태에 포함하지 않습니다. `summary.json`의 `performance`는 실행마다 달라지므로 재현 비교에는 `final-state.json` 또는 이벤트 로그를 사용합니다.

## 개발 검증

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build --workspace --all-features --release --locked
```

Windows에서 실행 파일의 CPU 시간·작업집합과 Tick별 구간 시간을 측정하려면 Release 빌드 후 다음 스크립트를 사용합니다. 같은 출력 디렉터리의 같은 이름 측정 파일은 갱신됩니다.

```powershell
.\scripts\benchmark.ps1
.\scripts\benchmark.ps1 -Population 100 -WithEvents
.\scripts\benchmark.ps1 -Repetitions 3 -OutputDirectory runs\benchmarks\phase4
.\scripts\benchmark.ps1 -Config examples\phase5.json -Repetitions 3 -OutputDirectory runs\benchmarks\phase5
```

테스트는 각 crate와 앱의 `tests/`에 배치합니다. 100명·1,000 Tick 연속 실행, 이벤트 단위 결정론, 화폐/자원 보존, 주문 예약·부분 체결·만료, 잘못된 행동, 기억 검색/보관, worker 제어와 CLI 파일 출력을 검사합니다. LLM 테스트는 Rust로 작성한 로컬 HTTP 서버로 실제 어댑터·타임아웃·동시성·예산·대체·결정 재적용을 검증합니다. 실제 유료 API나 설치된 Ollama 모델을 호출하지 않습니다.

2026-11-10 v0.5.0 검증에서 기본 구성 **103개**, 전체 기능 구성 **120개** 테스트가 통과했습니다. 새 기능을 켠 100명·1,000 Tick에서 협상 거래 2,685건, 공동 생산 250건, 목표 변경 182건이 발생했고, 연속 실행·파일/SQLite 복원·JSONL/SQLite Replay의 전체 상태가 같았습니다. 1,000명·1,000 Tick의 새 기능 포함 Release 실행은 3회 중앙값 **4.06초**였습니다. GUI 자동 60 Tick 실행·종료는 통과했고 초기 화면을 확인했으나 클릭 검증은 창 활성화 도구 오류로 완료하지 못했습니다. 측정 조건·한계와 이전 버전 이력은 [검증 기록](docs/validation.md)에 있습니다.

힙 할당을 추적하려면 `cargo run -p agentsociety-cli --profile profiling --example heap_profile -- --agents 100 --ticks 50 --out runs/profiles/heap.json`을 사용합니다. 이 독립 예제만 DHAT allocator를 사용하며 추적 시간은 일반 실행 속도와 구분합니다. 원시 스택 분석과 지표 정의는 [실험·성능 측정](docs/experiments.md)에 있습니다.

문서: [아키텍처](docs/architecture.md), [Tick 규칙](docs/simulation.md), [경제 모델](docs/economy.md), [에이전트와 정책](docs/agents.md), [협상·협력·목표 적응](docs/interactions.md), [조직·장기 계약](docs/organizations.md), [시나리오](docs/scenarios.md), [GUI](docs/desktop.md), [기억](docs/memory.md), [LLM 연동](docs/llm.md), [저장·복원](docs/persistence.md), [실험 비교](docs/experiments.md), [진행 현황](docs/roadmap.md), [검증 기록](docs/validation.md).


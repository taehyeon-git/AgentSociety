<h1 align="center">🌐 AgentSociety</h1>

<p align="center">
  <strong>에이전트의 선택이 경제와 사회를 만드는 시뮬레이터</strong><br>
  Rust 기반 · CLI & Desktop · 재현 가능한 실험
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/version-0.6.0-2563eb?style=flat-square" alt="Version 0.6.0"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-1.95%2B-dea584?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust 1.95 이상"></a>
  <a href="docs/architecture.md"><img src="https://img.shields.io/badge/docs-project_guide-0f766e?style=flat-square" alt="프로젝트 문서"></a>
</p>

<p align="center">
  <a href="#-빠른-시작">빠른 시작</a> ·
  <a href="#-주요-기능">주요 기능</a> ·
  <a href="#-문서">문서</a> ·
  <a href="#-개발-및-검증">개발 및 검증</a>
</p>

---

**AgentSociety**는 에이전트가 생산·거래·소비하고, 관계를 쌓고, 협상과 공동 생산에 참여하는 사회·경제 시뮬레이터입니다. 가격과 자산 분포가 어떻게 달라지는지 관찰하고, 공급 충격이나 세금·재분배 정책의 결과를 비교할 수 있습니다.

기본 규칙 모드는 **API 키나 LLM 없이 오프라인으로 실행**됩니다. 필요할 때 Ollama 또는 OpenAI 호환 모델을 연결할 수 있습니다.

## 🚀 빠른 시작

**준비:** Rust **1.95 이상**과 C 컴파일러가 필요합니다. Windows는 C++ Build Tools와 Windows SDK, Linux/macOS는 해당 플랫폼의 네이티브 빌드 도구를 사용합니다. 첫 빌드에는 의존성 다운로드가 필요합니다.

```sh
git clone https://github.com/taehyeon-git/AgentSociety.git
cd AgentSociety
```

### 🖥️ 데스크톱에서 둘러보기

```sh
cargo run --release --locked -p agentsociety-desktop -- --config examples/complete.json
```

100명의 에이전트가 있는 세계를 일시정지 상태로 엽니다. **Run**으로 실행하고 **Pause / Step**으로 멈추거나 한 Tick씩 진행하세요. 지도에서 에이전트를 선택하면 자산·기억·관계를 확인할 수 있습니다.

저장·불러오기와 시나리오 변경은 [데스크톱 사용법](docs/desktop.md)을 참고하세요.

### ⌨️ 터미널에서 실행하기

```sh
cargo run --release --locked -p agentsociety-cli -- run --config examples/complete.json --ticks 1000
```

같은 설정으로 1,000 Tick을 실행하고 결과를 출력합니다. 전체 옵션은 `cargo run -p agentsociety-cli -- --help`, 파일 출력과 설정 우선순위는 [CLI 사용 안내](docs/persistence.md#cli-사용)에서 확인할 수 있습니다.

> **설정 선택** — [complete.json](examples/complete.json)은 협상·목표 적응·조직·계약·시나리오를 함께 켠 예제입니다. 가벼운 실행은 [tiny.json](examples/tiny.json), 확장 기능을 끈 기본 모델은 [default.json](configs/default.json)을 사용하세요.

## ✨ 주요 기능

| 영역 | 할 수 있는 일 |
| :--- | :--- |
| **시장과 경제** | 네 직업·네 자원의 생산·소비, 주문장 거래, 가격·거래량·자산·Gini 관찰 |
| **에이전트와 관계** | 성격·욕구 기반 결정, 기억 기록·검색, 거래 기반 신뢰, 협상·공동 생산·목표 적응 |
| **조직과 계약** | 조직 설립·가입·탈퇴, 공동 재산과 배당, 보증금을 갖춘 장기 납품 계약 |
| **정책과 시나리오** | 공급 충격·재고 손실·판매세·재분배, 현재 상태에서 미래 일정만 바꾸는 분기 |
| **저장과 재현** | JSON 체크포인트, SQLite 복구·이력 조회, JSONL/SQLite 이벤트 Replay |
| **관찰과 비교** | 데스크톱 지도·그래프·타임라인, 시나리오·시드·인구·정책별 실험, JSON/CSV 출력 |

매 Tick 화폐·자원 수지와 예약·정산 상태를 검사합니다. 재현 비교에는 같은 엔진·의존성 버전, 설정·시드·외부 결정 입력을 사용하며, 실행 시간은 비교 대상에서 제외합니다. [Tick과 재현 규칙 →](docs/simulation.md)

<details>
<summary><strong>💾 저장한 실행 이어가기와 Replay</strong></summary>

```sh
cargo run --release --locked -p agentsociety-cli -- run --config examples/complete.json --ticks 500 --checkpoint-out runs/demo/checkpoint.json --events runs/demo/events.jsonl
cargo run --release --locked -p agentsociety-cli -- resume runs/demo/checkpoint.json --ticks 500
cargo run --release --locked -p agentsociety-cli -- replay runs/demo/events.jsonl --json
```

`resume`의 `--ticks`는 추가로 진행할 Tick 수입니다. 위 Replay는 첫 명령이 기록한 500 Tick을 검증합니다. 기존 출력 파일은 기본적으로 보호합니다.

SQLite 복구, 출력 형식과 버전 호환성은 [저장·복원](docs/persistence.md), 같은 상태에서의 시나리오 비교는 [분기 실행](docs/scenarios.md)을 참고하세요.

</details>

<details>
<summary><strong>🤖 선택적으로 LLM 연결하기</strong></summary>

```sh
cargo run --release --locked -p agentsociety-cli --features llm -- --agents 100 --ticks 100 --policy hybrid --llm-agents 4 --llm-config examples/llm-ollama.json
```

Ollama 서버를 실행하고 [설정 파일](examples/llm-ollama.json)의 `model`을 설치된 모델 이름에 맞춥니다. 앞쪽 ID의 4명은 Hybrid 정책을 사용하고 나머지는 규칙 정책을 유지합니다.

[OpenAI 호환 설정](examples/llm-openai-compatible.json)도 제공합니다. API 키는 지정한 환경변수에서만 읽으며 설정 파일에 넣지 않습니다. 호출 예산·동시성·시간 제한을 적용하고 실패 시 규칙 정책으로 대체합니다. [LLM 연동 안내 →](docs/llm.md)

</details>

## 📚 문서

관심 있는 주제부터 읽을 수 있도록 나누었습니다.

| 알아볼 내용 | 문서 |
| :--- | :--- |
| **사용과 실험** | [데스크톱](docs/desktop.md) · [저장·복원·Replay](docs/persistence.md) · [실험 비교와 성능 측정](docs/experiments.md) |
| **경제와 정책** | [경제 모델](docs/economy.md) · [협상·협력·목표 적응](docs/interactions.md) · [조직·장기 계약](docs/organizations.md) · [시나리오와 분기](docs/scenarios.md) |
| **에이전트의 결정** | [에이전트와 정책](docs/agents.md) · [기억](docs/memory.md) · [LLM 연동](docs/llm.md) |
| **내부 구조** | [아키텍처](docs/architecture.md) · [Tick 실행과 재현](docs/simulation.md) |
| **프로젝트 현황** | [구현 현황](docs/roadmap.md) · [검증 기록](docs/validation.md) |

현재 엔진은 **0.6.0**, 상태·이벤트 포맷은 **6**입니다. 이전 버전 출력의 자동 변환은 지원하지 않습니다. 모델의 가정과 결과 해석 범위는 [경제 모델](docs/economy.md)에 설명합니다.

## 🔧 개발 및 검증

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build --workspace --all-features --release --locked
```

Release 빌드 후 PowerShell 7.2 이상에서 저장·재개·Replay·분기·비교 실험을 한 번에 검증할 수 있습니다.

```powershell
.\scripts\verify-release.ps1
```

**v0.6 검증 기록:** Windows x64 · Rust/Cargo 1.100.0에서 기본 구성 **124개**, 전체 기능 구성 **143개** 테스트가 통과했습니다. 연속 실행·저장 후 재개·Replay의 상태 일치와 실제 Windows 창의 실행·저장·복원·시나리오 조작도 확인했습니다.

검증 시점·조건·성능 수치와 한계는 [검증 기록](docs/validation.md)을, 벤치마크와 힙 할당 추적 방법은 [성능 측정 안내](docs/experiments.md)를 참고하세요. LLM 자동 테스트는 로컬 모의 HTTP 서버를 사용하며 실제 모델의 응답 품질 검증과 구분합니다.

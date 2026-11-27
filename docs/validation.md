# 검증 기록

## 2026-11-27 — v0.6 조직·시나리오·저장 흐름 재검증

엔진 **0.6.0**, 상태·이벤트 포맷 **6**입니다. 현재 도구 체인은 Rust/Cargo **1.100.0**, Windows x64입니다. 아래 이력의 v0.5 수치와 구분합니다. 소스와 문서는 공개 저장소에 보관하고, 대용량 이벤트·SQLite·빌드·측정 원본은 `.gitignore`의 `runs/`와 `target/` 아래에 보관합니다.

### 빌드와 자동 테스트

기본 구성 테스트 **124개**, 전체 기능 구성 테스트 **143개**가 통과했고, 실패·무시 테스트는 0개입니다. 기본 구성과 전체 기능 구성 모두 `cargo clippy --workspace --all-targets ... -- -D warnings`를 통과했습니다. 로그는 `runs/final-20261127/tests-default-final.log`, `tests-all.log`, `clippy-default.log`, `clippy-all.log`입니다. 기존 123/142개 테스트에 외부 의도·시나리오 충격·Replay 경계 회귀 1개를 추가했습니다. 마지막 `cargo fmt --all -- --check`도 통과했습니다 (`fmt-final.log`). `cargo build --workspace --all-features --release --offline --locked`도 성공했습니다 (`build-release.log`).

Rust 1.100으로 변경되어 의존성을 다시 빌드했습니다. 제한 환경에서 기본 구성 재빌드가 장시간 지연되어 그 실행을 중단했고, 동일한 오프라인·잠금 의존성 명령을 제한 밖에서 재실행한 최종 로그만 완료 근거로 사용합니다. 중단 로그 `tests-default.log`와 최종 통과 로그를 구분합니다.

### 연속 실행과 복원·재생

`examples/complete.json`, 100명, seed 42, 1,000 Tick을 실행했습니다. 협상 거래 1,796건, 2인 공동 생산 38건, 목표 변경 211건, 조직 15개, 다자 공동 생산 1,788건, 계약 납품 1,499건·불이행 85건, 세금 447,711·재분배 444,898·금고 2,813을 기록했습니다. 최종 기아 인원은 0명, Gini는 0.5286522006144481입니다. 이는 이 시드·모델의 관측 결과이며 정책의 일반적인 우월성을 뜻하지 않습니다.

다음 다섯 경로의 전체 상태 파일 SHA-256이 모두 `d2d89fd4da1badc9a8bf6a4e526326be922d45d6212776d9d89339608bf6eb98`로 일치했습니다.

1. 1,000 Tick 연속 실행
2. 500 Tick 체크포인트에서 추가 500 Tick
3. 500 Tick SQLite의 독립 복사본에서 추가 500 Tick
4. 연속 실행 JSONL의 전체 Replay
5. 이어 실행한 SQLite의 전체 Replay

두 Replay는 각각 1,000 Tick·420,515개 이벤트를 검증했습니다. Replay가 계산한 compact JSON 상태 SHA-256은 `f171efb78cf6a99da5d7a920cf21b8f4cbf4d3507144b75e4330535f3a2f4061`입니다. 내보낸 파일은 pretty JSON이므로 파일 바이트의 해시와 구분해야 합니다.

재현 명령은 `scripts/verify-release.ps1`에 있습니다. 기존 출력 디렉터리를 거절하고, 실패 시 즉시 중단하며, 소스 체크포인트·SQLite·입력 설정·실행 파일의 불변성도 확인합니다. 상세 원본 위치는 `runs/final-20261127/release-final/`입니다.

### 시나리오 분기와 비교 실험

500 Tick 체크포인트에서 `branch-after-500.json`으로 미래 일정만 교체하여 500 Tick을 추가했습니다. 새 JSONL/SQLite의 Replay는 각각 500 Tick·213,473개 이벤트를 검증했고, 분기 상태와 두 Replay 상태의 파일 SHA-256은 `4c7e146374cd5c7b0c3ecb1652c46b5557c7617ccc20f360e6373d751486d6f0`로 일치했습니다. 원본 체크포인트와 원본 SQLite의 해시는 실행 전후 그대로였습니다. 분기 결과는 기존 일정 결과와 다릅니다.

100명·1,000 Tick, 규칙 정책, 시드 42·43·44로 다음 3개 조건을 교차한 **9개 실행**이 모두 완료됐습니다. 비교는 시나리오별 3회 평균이며 통계적 유의성을 검증한 인과 추정이 아닙니다.

| 시나리오 | 평균 Gini | 평균 개인 평가 자산 |
| --- | ---: | ---: |
| 개입 없음 (baseline) | 0.440691 | 37,785.52 |
| 공급·재고 위기 (crisis) | 0.434584 | 38,844.20 |
| 위기·세금·재분배 (redistribution) | 0.495901 | 25,541.03 |

9개 조합의 완료 Tick·인구·정책·시드·시나리오 이름, 체크포인트 존재, 상태 해시와 CSV 9행을 확인했습니다. 가격이 평가 자산에 영향을 주므로, 위기 조건의 평균 자산이 높다는 이유만으로 생활 수준이 개선됐다고 해석할 수 없습니다. 모든 명령은 종료 코드 0이며 자세한 지표는 `release-final/matrix/report.json`과 `verification.json`에 있습니다.

### Release 성능 반복 측정

Rust 1.100으로 새로 만든 전체 기능 Release CLI를 사용했습니다. 다른 빌드·실험과 겹치지 않게 각 조건을 순서대로 3회 실행했습니다. seed 42, 1,000 Tick, 규칙 정책, JSONL 기록 없음이며 아래 시간은 엔진의 `performance.elapsed_ms`입니다. 프로세스 시작 시간과 빌드 시간은 포함하지 않습니다.

| 구성 | 인구 | 중앙값 | 최소~최대 | 3회 중 최대 관측 작업집합 |
| --- | ---: | ---: | ---: | ---: |
| `examples/complete.json` | 100 | 1.53초 | 1.35~1.91초 | 9.63 MiB |
| `examples/complete.json` | 1,000 | 18.07초 | 17.26~19.19초 | 32.48 MiB |
| 기본 구성 (조직·개입 없음) | 1,000 | 3.71초 | 3.42~3.89초 | 29.92 MiB |

전체 기능 조건은 조직·계약·시나리오·적응 등 수행하는 작업이 더 많으므로 기본 조건과 동일한 작업량의 속도 비교가 아닙니다. 1,000명 전체 기능 조건은 세 실행 모두 96,773건의 거래를 기록했습니다. 작업집합은 Windows 프로세스를 약 10ms 간격으로 관측한 값이며 힙 할당량과 다릅니다. 환경별 속도를 보장하는 수치가 아니고, 앞선 버전의 다른 기능 구성과도 직접 비교하지 않습니다.

원본은 `runs/final-20261127/benchmarks-enabled/baseline-measurements.json`, `benchmarks-default/baseline-measurements.json`과 각 실행 요약에 있습니다. 다음 명령으로 재현합니다.

```powershell
.\scripts\benchmark.ps1 -Config examples/complete.json -Population 100,1000 -Ticks 1000 -Repetitions 3 -OutputDirectory runs/benchmarks/complete
.\scripts\benchmark.ps1 -Population 1000 -Ticks 1000 -Repetitions 3 -OutputDirectory runs/benchmarks/default
```

최종 Release를 대상으로 현재 `verify-release.ps1`도 전체 실행하여 `release-final/verification.json`의 `passed: true`와 10개 명령의 종료 코드 0을 확인했습니다. 이전 빌드와 새 빌드의 전체 상태 파일 해시도 동일합니다. 스크립트의 오류 종료 코드 기록·단일 원소 배열 직렬화·기존 결과 폴더 보호 검사도 별도로 통과했습니다 (`cli-script-checks/checks.json`).

### GUI와 외부 모델 검증 범위

전체 기능 예제로 새 Release 네이티브 GUI를 자동 시작하여 60 Tick 뒤 종료 코드 0, 비어 있는 stderr를 확인했습니다 (`gui-final.*.log`). OS 실제 클릭 검증은 Windows 자동화의 `list_windows`가 반복해서 시간 초과했고, 커널 초기화 및 별도 CUA 연결도 시간 초과하여 완료하지 못했습니다. 이를 통과로 기록하지 않습니다. egui 포인터·키보드 입력 회귀 테스트와 네이티브 렌더러 자동 실행은 각각 별도의 검사입니다.

egui 입력을 통한 재시작·저장·불러오기·실패 보존 및 시나리오 분기 테스트는 통과했습니다. LLM 검증은 로컬 모의 HTTP 서버와 기록된 외부 결정으로 수행합니다. 실제 유료 API 또는 설치된 Ollama 모델의 판단 품질은 검증하지 않습니다. 외부 의도는 직전 완료 상태에서 만들어질 수 있으므로 이후 재고 충격으로 무효가 된 의도의 거절과 Replay 일치를 별도 회귀 테스트로 추가했고 이 테스트도 통과했습니다. 직업 변경·신용·학습 정책·이전 포맷 마이그레이션 등 모델 밖의 범위는 [roadmap.md](roadmap.md)에 명시합니다.

## 2026-11-10 — Phase 5 협상·공동 생산·목표 적응

엔진 **0.5.0**, 상태/이벤트 포맷 **5**입니다. 체크포인트·SQLite 스키마는 각각 1을 유지합니다. 기본 구성 **103개**, 전체 기능 **120개** 테스트가 통과했으며 실패·무시 테스트는 0개입니다. fmt 검사, 기본/전체 기능 Clippy의 `-D warnings`, 전체 Workspace의 전체 기능 Release 빌드도 통과했습니다. 새로운 외부 의존성은 추가하지 않았습니다.

검사에는 작업 폴더의 Cargo 캐시와 `--offline --locked`를 사용했습니다. `runs/validation/phase5-fmt.log`, `phase5-clippy-default.log`, `phase5-clippy-all.log`, `phase5-tests-default.log`, `phase5-tests-all.log`, `phase5-release-build.log`, `phase5-test-counts.json`에 원문을 보관합니다. 아래 Phase 1~4는 당시 버전의 이력이며 현재 검증 범위는 이 절을 기준으로 읽습니다.

### 추가 회귀 검사

- 예약 자산의 중복 사용 금지, 중앙 주문 만료 후 제안 담보 유지, 정확한 금액·수량·개인 매수가 정산.
- 역제안의 소유자 전환·고정 기한·3회 한도, 같은 Tick 응답·제3자 응답·중복 상대·자기 제안 거절.
- 실패한 역제안·부족한 자금·정산 오버플로·공동 생산의 입력 부족/출력 오버플로에서 두 계좌와 제안이 그대로 유지됨.
- 공동 생산의 실제 Metal/Wood 소비·Goods 배분·양쪽 노동 비용, 만료 시 준비 노동의 비반환, 거절된 제안을 거래로 기억하지 않음.
- 중앙 거래와 협상 거래의 합산 가중 가격, 실제 성공에만 발생하는 성과·관계·기억.
- 기아 우선 목표 변경, 식량 비축의 변경 억제 구간, 거래·협력 성과 조건, 기억 보관을 꺼도 같은 목표·사건·경제 상태.
- 100명·1,000 Tick의 같은 시드 실행, 500 Tick 복원 후 매 이벤트·전체 상태 일치. 독립 기능, 수명 1, 인구 0/1, 현금 0에서도 복원.
- 진행 중 제안·예약·기한·역할·누적 지표·목표 성과를 손상시킨 스냅샷 거절.
- 진행 중 협상과 공동 생산의 파일 체크포인트·SQLite 확정 꼬리 복구·JSONL/SQLite Replay 및 양쪽 참여자 조회.
- 실제 loopback HTTP의 모델 제안·수락·공동 생산, 제한된 제안 문맥, 없는 제안 수락의 규칙 대체, 체크포인트부터 서버 재호출 없는 재현.

### 실제 CLI 실행과 파일 비교

`examples/phase5.json`의 100명·시드 42·목표 검토 간격 50을 사용했습니다. 연속 1,000 Tick, 파일 체크포인트 500+500, SQLite 500+500, 이어 실행 JSONL Replay, SQLite 전체 Replay의 다섯 최종 상태는 **3,603,995바이트로 동일**합니다. Tick 500의 체크포인트에는 진행 중 제안 9개와 예약 현금 2,309가 있었습니다. 최종 Tick에는 제안 5개가 남아 있고 모두 유효 담보로 복원됩니다.

```text
pretty JSON 파일 SHA-256:
05AEA2576F118F270A561A7A2A784F4B6AAE54DDAB752F21C5904D5951A053B5
compact 상태 SHA-256:
e41627d056c4138d47aec399d6ebd4d191e3aef25f69140475ed0ad4fb9e078f
```

| 항목 | 결과 |
| --- | ---: |
| 전체 화폐 | 972,137 |
| 전체 거래 / 협상 체결 | 9,022 / 2,685 |
| 새 협상 제안 | 5,220 |
| 완료 공동 생산 | 250 |
| 실제 목표 변경 | 182 |
| 최종 Gini / 기아 인구 | 0.8244246810704752 / 0 |
| 전체 검증 이벤트 | 390,926 |
| Tick CSV 행 수 | 1,000 |
| 에이전트 0 전체 조회 | 4,538개, 제안·역제안·체결·목표 변경 포함 |

CSV의 새 지표를 합한 값이 누적 요약과 일치합니다. 인구 10/100 × 시드 42/43/44의 6개 실험을 실행했고, 2개 비교 그룹의 평균·실행별 CSV 값과 JSON을 대조했습니다. 100명·시드 42 실험의 상태 해시도 독립 실행과 같습니다. 이 한 시드 결과만으로 협력이 언제나 부나 평등을 높인다고 해석하지 않습니다.

실제 파일은 `runs/phase5/`, 검증 결과는 `runs/validation/phase5-release-acceptance.json`, `phase5-export-checks.json`에 있습니다. 실행 명령 기록은 `phase5-acceptance.ps1`입니다. 이전 Phase 4 파일은 그대로 보존했고 구 버전 CLI는 `.build-cache/releases/0.4.0/agentsociety-cli.exe`에 보관했습니다. 포맷 4의 자동 변환은 지원하지 않습니다.

### 성능과 GUI

같은 v0.5.0 Release에서 각 조건을 별도 프로세스로 3회 실행했습니다. 시드 42, 1,000 Tick, 기본 기억·관계 한도, Tick별 감사 활성화, 이벤트 파일 미기록이며 컴파일과 동시에 측정하지 않았습니다. 아래는 중앙값입니다.

| 인구 | 세 기능 끔 | 세 기능 켬 | 켠 상태의 관측 최대 작업집합 |
| ---: | ---: | ---: | ---: |
| 100 | 94.24 ms | 323.22 ms | 8.47 MiB |
| 1,000 | 1,345.21 ms | 4,064.17 ms | 19.70 MiB |

기능을 켠 1,000명 실행의 화폐는 9,967,494, 전체 거래 91,845건, 협상 체결 26,621건, 공동 생산 3,213건, 목표 변경 1,829건입니다. 결정 단계는 대표 실행에서 평균 Tick당 약 2.52ms입니다. 새로운 관찰·상대 선택의 비용이 포함되며 기능을 끈 실행과 행동 결과도 다릅니다. 이 비교는 같은 알고리즘의 최적화 전후 측정이 아니며, 이전 Phase의 벽시계 기록과 성능 향상률을 계산하지 않습니다. 작업집합은 10ms 간격 표본으로 정확한 힙 할당량과 다릅니다.

원문은 `runs/benchmarks-phase5-enabled/`, `runs/benchmarks-phase5-disabled/`와 `runs/validation/phase5-benchmark-comparison.json`입니다. `scripts/benchmark.ps1 -Config examples/phase5.json`으로 새 기능 조건을 다시 측정할 수 있습니다. 이번 단계에서 DHAT 할당 추적은 다시 수행하지 않았습니다.

Release GUI는 `--config examples/phase5.json --auto-start --exit-after-ticks 60`으로 실행해 **3,724ms, 종료 코드 0, stderr 0바이트**를 확인했습니다. 컴퓨터 사용 도구로 새 설정 행·상호작용 지표·제안 Inspector가 포함된 초기 화면도 확인했습니다. 이후 창 활성화가 `failed to activate captured window`로 실패했고 새 창 선택 후 1회 재시도도 같아 **새 설정 클릭·Restart 경로의 시각 검증은 미완료**입니다. worker의 기본 제어 자동 테스트와 네이티브 자동 실행은 이 제한과 구분합니다. 원문은 `phase5-gui-smoke.json`, `phase5-gui-visual.json`입니다.

## 2026-11-10 — Phase 4 복원·Replay·실험과 성능 개선

엔진 **0.4.0**, 상태/이벤트 포맷 **4**, 체크포인트·SQLite 스키마 각각 **1** 당시의 기록입니다. 기본 구성 **82개**, `--all-features` 구성 **98개** 테스트가 통과했습니다. 실패·무시 테스트는 없습니다. 로그에서 집계한 결과는 `runs/validation/phase4-test-counts.json`입니다.

```powershell
$env:CARGO_HOME = Join-Path (Get-Location) '.build-cache/cargo'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --offline --locked -- -D warnings
cargo test --workspace --all-features --offline --locked
cargo test --workspace --offline --locked
cargo build --workspace --all-features --release --offline --locked
```

fmt, 기본/전체 기능 Clippy가 경고 없이 통과했고, 전체 Workspace의 전체 기능 Release 빌드도 종료 코드 0으로 완료했습니다. 자동 테스트 로그는 `phase4-final-tests.log`, `phase4-final-offline-tests.log`, 검사 로그는 `phase4-final-fmt.log`, `phase4-final-clippy.log`, `phase4-final-offline-clippy.log`, 최종 빌드 로그는 `phase4-final-release-build.log`입니다. 모두 `runs/validation/` 아래에 보관합니다. 번들 SQLite의 C 코드도 작업 폴더의 MSVC 도구로 실제 컴파일했습니다. 새 Cargo 의존성 아카이브는 레지스트리 SHA-256을 확인하고 사용했습니다.

최종 Release GUI도 `--agents 100 --seed 42 --auto-start --exit-after-ticks 10`으로 실행하여 **1,458ms, 종료 코드 0, stderr 0바이트**를 확인했습니다. 이는 실제 네이티브 렌더러 초기화·10 Tick 진행·자동 종료 검사이며 모든 화면 조작의 수동 검사를 대신하지 않습니다. 원문은 `runs/validation/phase4-gui-smoke.json`입니다. README와 docs의 11개 Markdown 파일에서 로컬 링크가 가리키는 파일의 존재도 확인했습니다.

### 복원과 실패 처리

- 엔진의 1,000 Tick 연속 실행과 500 Tick 저장·복원 후 500 Tick 실행의 매 이벤트와 전체 상태가 같습니다. 인구 0·1·다수, 보관 한도 0도 확인했습니다.
- 버전, ID, 계좌, 예약, 자원 수지, 이력, 관계·기억 참조, RNG 위치 등의 잘못된 스냅샷을 거절합니다. 체크포인트 체크섬·알 수 없는 상위 필드·기존 출력 보호도 검사했습니다.
- JSONL의 잘린 줄, 완료 footer 누락, 변경된 이벤트, 순서 오류, 추가 꼬리 데이터를 거절합니다. 파일 경계만 잘못된 경우 실행 전 거절하고, 재생성 이벤트가 달라진 경우 엔진을 실패 상태로 고정합니다.
- SQLite 체크포인트 뒤의 확정 이벤트 복구, 전체 Replay, 양쪽 거래 당사자 조회, 변조된 이벤트·최신 체크포인트 거절을 확인했습니다. SQL trigger로 쓰기 실패를 주입하여 이벤트·정책·체크포인트·현재 위치가 함께 롤백되는지 검사했고, 오래된 writer의 추가 기록도 거절했습니다.
- **실제로 실행 중인 CLI 프로세스를 종료**한 뒤 마지막 확정 Tick을 복구하고 같은 Tick 수의 연속 실행과 상태를 비교했습니다. 이 검사는 프로세스 종료 복구이며 전원 차단·장치 고장 시의 물리적 내구성을 실험한 것은 아닙니다.
- 두 독립 프로세스에 걸쳐 LLM 호출 예산 2회와 4 Tick 간격이 보존됩니다. 이후 서버를 종료하고 키를 제거한 상태에서도 기록을 Replay하여 같은 상태를 얻었습니다. 기본 빌드에서도 외부 정책 로그를 읽을 수 있습니다.
- 서로 다른 시드·인구의 실험 결과와 집계 평균, 저장된 설정·체크포인트·이벤트, 두 행렬 실행의 상태 해시를 비교했습니다. 알 수 없는 설정과 기존 결과 경로는 실행 전에 거절합니다.

### 실제 Release 실행: 100명·1,000 Tick

seed 42, RuleBased, 기본 기억·관계 보관 설정입니다. 다음 다섯 경로를 독립 CLI 프로세스로 실행했습니다.

1. 1,000 Tick 연속 실행
2. 500 Tick 파일 체크포인트에서 500 Tick 추가 실행
3. 500 Tick SQLite에서 500 Tick 추가 실행
4. 이어 실행 구간의 JSONL Replay
5. SQLite의 전체 1,000 Tick Replay

다섯 최종 상태 파일은 바이트 단위로 같으며 파일 SHA-256은 다음과 같습니다.

```text
835FBF368C4C8BC31D609BAB8304BF38866C32356620E2CFE2BD155F63A17DD9
```

| 확인 항목 | 결과 |
| --- | ---: |
| 최종 화폐 / 거래 체결 | 972,137 / 8,010건 |
| 최종 Gini | 0.8435356331856072 |
| SQLite 검증 이벤트 | 392,842개 |
| Tick 501~1,000 JSONL 구간 검증 이벤트 | 191,681개 |
| 에이전트 0, Tick 990~1,000 조회 | 44개 |
| 인구 10·100 × 시드 42·43·44 실험 | 6개 실행, 2개 비교 그룹 |

실험 행렬의 100명·seed 42 결과도 독립 실행과 같습니다. Replay·실험의 compact JSON 상태 해시는 `861926ce4fde9f2e52fe5fd50236c50237a4052c687e6e13354b160a05d5d332`입니다. 이 값은 위의 pretty JSON 파일 바이트 해시와 직렬화 범위가 다릅니다.

원문은 `runs/phase4/`의 상태·요약·JSONL·SQLite·CSV와 `experiments/`에, 비교 결과는 `runs/validation/phase4-release-acceptance.json`에 있습니다. 추가 검증에 사용하는 공급자는 Rust loopback HTTP 서버이며 실제 유료 API나 설치된 Ollama 모델의 품질·지연·요금 검증은 여전히 범위 밖입니다.

### 측정 후 시장 탐색 최적화

같은 v0.4.0, seed 42, 1,000 Tick, RuleBased, 기억 한도 64와 기본 관계 설정, 매 Tick 보존 검사를 사용했습니다. 최적화 전과 후 각각 **3개의 독립 Release 프로세스**를 측정했고 아래는 중앙값입니다. 컴파일은 측정과 동시에 수행하지 않았습니다.

| 인구 | 전 실행 시간 | 후 실행 시간 | 전 평균 시장 | 후 평균 시장 | 후 관측 최대 작업집합 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 11.62 ms | 11.36 ms | 0.00254 ms | 0.00264 ms | 6.594 MiB |
| 100 | 227.33 ms | 185.06 ms | 0.07096 ms | 0.04714 ms | 8.074 MiB |
| 1,000 | 5,417.34 ms | 3,127.44 ms | 2.84759 ms | 0.78235 ms | 17.680 MiB |

1,000명에서 시장 처리 구간이 평균 Tick 시간의 약 55%인 것을 확인한 뒤, 정렬된 매도 목록의 가장 싼 유효 가격대만 탐색하도록 변경했습니다. 신뢰·시간·주문 ID의 동률 해소 규칙은 그대로입니다. 실행 시간 중앙값은 **42.27% 감소**, 시장 시간 중앙값은 약 **72.53% 감소**했습니다. 전체 프로세스 CPU 시간 중앙값은 **5,046.88 → 3,031.25ms**입니다. 작은 인구의 짧은 실행은 초기화와 스케줄링 잡음의 비중이 크므로 작은 차이를 일반화하지 않습니다.

최적화 전후 1,000명·1,000 Tick의 상태 파일 해시는 모두 `B8B9E8E51B2CA43EF399C39D080837E8DC845B3DBEE8F3DC69AD630E9C227ADD`입니다. 두 실행 모두 화폐 9,967,494, 거래 73,839건, 이벤트 3,912,542개를 기록합니다. 회귀 테스트는 싼 자기 주문 건너뛰기, 같은 가격에서 신뢰 우선, 부분 체결 뒤 다음 가격대 진행과 잔여 예약 해제를 확인합니다.

최적화 후 중앙 실행의 Tick당 구간은 시장 0.782ms, 행동 0.748ms, 기억 0.517ms, 사회관계 0.255ms, 결정 0.242ms, 통계·감사 0.191ms, 생활 소비 0.184ms, 환경 0.009ms였습니다. 이는 벽시계 구간 측정이며 CPU 스택 샘플링은 아닙니다. `process_cpu_ms`는 별도로 OS에서 얻은 전체 프로세스 CPU 시간입니다.

100명에서 JSONL 기록을 켠 3회 측정의 실행 시간 중앙값은 **884.34ms**, 이벤트 기록 시간은 **645.07ms**, 파일 크기는 **70,836,380 bytes**, 관측 최대 작업집합은 **11.941MiB**입니다. 기록에는 시작 상태·Tick 확정·완료 체크포인트가 포함됩니다. 거래·화폐·결정론적 상태는 비기록 실행과 같습니다. SQLite 성능을 이 JSONL 수치로 대신 평가하지 않습니다.

원시 파일은 `runs/benchmarks-phase4-before/`, `runs/benchmarks-phase4-after/`에, 정리한 비교는 `phase4-benchmark-comparison.json`과 `phase4-optimization-state-comparison.json`에 있습니다. 작업집합은 명목상 10ms 간격의 표본이며 정확한 최대 힙 할당량이나 다른 장비의 성능 보장이 아닙니다.

### 별도 실행 파일의 힙 할당 추적

일반 CLI·GUI의 allocator는 바꾸지 않았습니다. `apps/cli/examples/heap_profile.rs`만 DHAT를 사용하며 Release 최적화와 소스 위치 정보를 가진 `profiling` 프로필로 빌드했습니다. 초기화·전체 Tick·월드 해제를 추적하고 결과 직렬화는 제외했습니다. seed 42, 기본 기억·관계 설정이며 두 조건을 각각 한 번 실행했습니다.

| 조건 | 총 할당 횟수 | 누적 할당 바이트 | 최대 동시 생존 바이트 | 종료 직전 월드 보관 바이트 | 월드 해제 뒤 바이트 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100명·50 Tick | 64,762 | 20,375,585 | 950,569 | 766,265 | 0 |
| 1,000명·100 Tick | 1,237,900 | 387,682,257 | 9,884,745 | 8,173,881 | 0 |

1,000명 조건의 주요 누적 할당 지점은 Tick 이벤트 벡터 **17.50%**, sequence를 붙인 EventRecord 벡터 **16.57%**, 행동 적용 중 이벤트 생성 지점 **8.71%**, 생활 소비의 개인별 이벤트 벡터 **6.60%**였습니다. 이는 원시 호출 스택을 소스 위치와 대조한 결과입니다. 누적 할당량에는 반복 해제·재할당이 포함되므로 약 370MiB를 동시에 보관했다는 뜻이 아닙니다. 최대 동시 생존 힙은 약 **9.43MiB**였고, 추적 범위 안에서 월드 해제 뒤 잔여량은 0이었습니다. 이 한 번의 측정이 모든 실행의 누수 부재를 보장하지는 않습니다.

추적 시간은 100명 약 84ms, 1,000명 약 2,815ms였으며 스택 수집 오버헤드를 포함합니다. 일반 Release 성능과 비교하지 않습니다. 추적 내부 메모리·프로세스 런타임 전체의 작업집합과도 다른 측정입니다. 향후 할당 개선을 평가할 지점은 위의 임시 이벤트 벡터입니다.

원시 추적은 `runs/profiles/phase4-100-50.json`, `phase4-1000-100.json`, 요약은 `runs/validation/phase4-heap-summary.json`, `phase4-heap-1000-summary.json`, 주요 스택은 `phase4-allocation-sites.json`, 빌드 로그는 `phase4-profiler-build.log`에 있습니다. 재실행 방법과 [DHAT 공식 문서](https://docs.rs/dhat/0.3.3/dhat/)는 [실험·성능 측정](experiments.md)에 연결합니다.

### 호환성과 남은 범위

v0.3 파일 마이그레이션, JSONL 중단 구간의 부분 복구, GUI의 저장·불러오기 화면, 실험 행렬 자체의 중도 재개는 제공하지 않습니다. 중단 지점 복구에는 SQLite를 사용합니다. SHA-256은 손상 검출용이며 서명이 아닙니다. 협상·협력·장기 목표 적응과 시장 충격은 Phase 5에 남깁니다.

## 2026-11-10 — Phase 2·3 검증

엔진 **0.3.0**, 상태/이벤트 포맷 **3**. 작업 시작 시 기존 전체 테스트는 40개였고, 기억과 선택적 LLM 연동을 추가한 뒤 기본 구성 **59개**, `--all-features` 구성 **74개**가 모두 통과했습니다. 실패·무시 테스트는 없습니다.

```powershell
$env:CARGO_HOME = Join-Path (Get-Location) '.build-cache/cargo'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo test --workspace --offline --locked
cargo clippy --workspace --all-targets --all-features --offline --locked -- -D warnings
cargo test --workspace --all-features --offline --locked
cargo build --workspace --all-features --release --offline --locked
```

두 구성 모두 Clippy 경고 없이 통과했고 전체 기능 Release 빌드도 종료 코드 0으로 완료했습니다. 현재 의존성은 작업 폴더의 `.build-cache/cargo` 캐시를 사용합니다. 기존 vendor 디렉터리는 새 LLM 의존성을 모두 포함하지 않으므로 로컬 설정을 다시 생성할 때 `scripts/setup-local-msvc.ps1 -SkipVendor`를 사용합니다. HTTPS 의존성의 C 컴파일에 필요한 Microsoft `clui.dll` 언어 리소스도 SHA-256을 확인한 공식 패키지에서 작업 폴더 안으로 설치하도록 스크립트를 보완했고, 수정한 설정 스크립트의 실제 재실행도 통과했습니다.

| 영역 | 전체 기능 구성 테스트 수 | 주요 검증 |
| --- | ---: | --- |
| core | 4 | 계좌 예약, 원자성, 오버플로 |
| world | 2 | 시드 초기화, 자원 부족 무변경 |
| economy | 10 | 가격 우선·동일 가격 신뢰 선호, 예약·정산·제조 |
| cognition | 15 | 규칙 정책 5개, 실제 로컬 HTTP를 사용하는 LLM 정책 10개 |
| analytics | 2 | Gini, 빈 사회 |
| social | 2 | 실제 체결 기반 관계, 제한된 관계 보관·분석 |
| memory | 9 | 실제 이벤트와 양자 거래 기억, 보관 한도, 결정론적 검색 |
| simulation | 19 | 연속 실행·보존·결정론 8개, 기억 4개, 결정 배치 4개, LLM 실행 3개 |
| CLI | 7 | 출력·경로 보호·프로세스 재현·어댑터 오류 5개, 실제 HTTP 설정·키 비노출 2개 |
| desktop | 4 | worker 제어 3개, LLM 대기 중 읽기·종료·예산 초기화 1개 |
| **합계** | **74** | **기본 구성은 59개** |

LLM 통합 검증은 Rust loopback HTTP 서버로 Ollama와 OpenAI 호환 요청, 엄격한 JSON, 불가능한 행동, 응답 크기, 오류·타임아웃, 동시성·호출 예산, Hybrid 조건과 토큰 집계를 확인했습니다. 비동기 응답을 고정 Tick의 정렬된 결정 배치로 적용하며, 그 배치를 네트워크 없이 다시 적용했을 때 이벤트·기억·최종 상태가 일치함을 검사합니다. 실제 유료 API나 설치된 Ollama 모델의 판단 품질·지연·비용은 측정하지 않았습니다.

네이티브 GUI에서 100명 실행과 거래/가격 갱신을 관찰했고, Pause 후 Tick **177→178**의 단일 Step을 확인했습니다. 기억을 추가한 화면에서 `Memories (1)` 렌더링도 확인했습니다. 추가 클릭 검사는 컴퓨터 제어 도구의 `window is not a usable app window` 오류와 이후 창 검색 실패로 중단했으므로, 모든 GUI 조작이나 실제 모델 응답의 화면 표시를 수동 검증했다고 해석하지 않습니다.

전체 기능 Release 실행 파일로 `agentsociety-desktop --agents 100 --seed 42 --auto-start --exit-after-ticks 10`을 실행한 네이티브 렌더러 검사도 **2,370ms, 종료 코드 0, stderr 없음**으로 완료했습니다. 이는 창 초기화와 자동 실행·종료 검사이며 모든 화면 조작을 대신하지 않습니다. 원문은 `runs/validation/phase3-gui-smoke.json`에 있습니다.

서버 연결 실패 설정을 사용한 LLM 정책 GUI도 10명·10 Tick 자동 종료 조건으로 실행해 **1,145ms, 종료 코드 0, stderr 없음**을 확인했습니다. 원문은 `runs/validation/phase3-llm-gui-smoke.json`입니다. 실제 모델 응답을 받아 화면을 검토한 검사는 아닙니다.

자동 테스트와 빌드 원문: `runs/validation/phase3-tests.log`, `phase3-offline-tests.log`, `phase3-clippy.log`, `phase3-release-build.log`. 생성 파일은 Git에서 제외됩니다.

### Release 실행과 재현성

100명·1,000 Tick·seed 42·RuleBased 정책으로 실행했습니다. 한 프로세스는 JSONL/CSV/요약/최종 상태를 기록했고 다른 프로세스는 JSONL/CSV 없이 같은 설정으로 실행했습니다. 두 최종 상태 파일의 SHA-256은 같습니다.

```text
3F873150667AC4B34FFD1BB2BC9E008F21D039263F8484219836B474C7081F99
```

| 지표 | 결과 |
| --- | ---: |
| 초기 = 최종 화폐 | 972,137 |
| 누적 거래 체결 / 대금 | 8,010건 / 2,883,132 |
| 평균 평가 자산 / 최종 Gini | 11,291.16 / 0.8435356 |
| 이벤트 / 거절 행동 / 최종 기아 | 392,842개 / 0건 / 0명 |
| 에이전트당 보관 기억 | 모두 64개, 전체 6,400개 |

| 자원 | 초기 | 생산 | 생활 소비 | 제조 투입 | 최종 | 최종 가격 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Food | 1,375 | 99,548 | 100,000 | 0 | 923 | 112 |
| Wood | 450 | 6,951 | 0 | 7,152 | 249 | 62 |
| Metal | 275 | 3,351 | 0 | 3,576 | 50 | 125 |
| Goods | 225 | 4,625 | 4,785 | 0 | 65 | 491 |

모든 Tick에서 보존 검사를 통과했으며 각 행의 `초기 + 생산 = 생활 소비 + 제조 투입 + 최종`이 성립합니다. Phase 1 이후 동일 가격에서의 신뢰 선호 등 모델 규칙이 바뀌었으므로 과거 Phase 1 수치와의 일치를 요구하지 않습니다. 기억 보관만 켜고 끌 때 경제 상태·이벤트·난수 위치가 달라지지 않는 것은 별도 회귀 테스트로 확인했습니다.

결과: `runs/phase3-final-state.json`, `phase3-reproduced-state.json`, `phase3-summary.json`, `phase3-reproduced-summary.json`, `phase3-events.jsonl`, `phase3-metrics.csv`. 해시 비교는 `runs/validation/phase3-reproducibility.json`에 있습니다. 이 상태 파일은 검사·비교용이며 파일 복원 명령은 아직 구현하지 않았습니다.

LLM 서버를 사용할 수 없는 `http://127.0.0.1:1`을 지정한 별도 Release 실행도 완료했습니다. 10명·10 Tick, 외부 정책 1명, 총 호출 예산 2회, 배치 제한 50ms에서 **호출 예약 2회, 외부 승인 0회, 규칙 대체 10회**를 기록했고 화폐 보존을 통과했습니다. 원문은 `runs/phase3-fallback-summary.json`과 `phase3-fallback-events.jsonl`입니다. 서버 실패와 예산 소진 이후에도 진행되는지 확인하는 검사입니다.

### 현재 기능의 Release 성능

전체 기능으로 빌드한 실행 파일의 RuleBased 모드, seed 42, 1,000 Tick, 기억 한도 64, 관계 한도 기본값, 매 Tick 보존 검사를 사용했습니다. 컴파일을 마친 뒤 `scripts/benchmark.ps1 -OutputDirectory runs/benchmarks/phase3`으로 각각 한 번 측정했습니다.

| 에이전트 | Tick 루프 총 시간 | 평균 Tick | 최대 Tick | 평균 시장 처리 | 에이전트-Tick/초 | 관측 최대 작업집합 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 12.13 ms | 0.01168 ms | 0.272 ms | 0.00326 ms | 824,572 | 6.742 MiB |
| 100 | 222.91 ms | 0.21936 ms | 2.438 ms | 0.07251 ms | 448,611 | 7.855 MiB |
| 1,000 | 5,536.55 ms | 5.29009 ms | 18.884 ms | 2.94279 ms | 180,618 | 17.254 MiB |

100명 JSONL 기록 측정은 총 **835.27ms**, 이벤트 직렬화/쓰기 **569.26ms**, 로그 **69,277,777 bytes**, 관측 최대 작업집합 **8.113MiB**였습니다. 거래 수와 화폐량은 비기록 실행과 같습니다. 1,000명에서는 시장 처리 구간이 평균 Tick의 약 56%를 차지합니다. 이는 구간 시간 측정이며 세부 CPU/할당 프로파일링은 아직 수행하지 않았습니다.

작업집합 표본은 인구별 6/15/186개, JSONL 실행은 37개입니다. 표본 간격은 명목상 10ms이지만 Windows 스케줄링에 따라 달라집니다. 단회 실행과 작업집합 표본은 정확한 최대 할당량이나 다른 장비의 성능 보장을 뜻하지 않습니다. 원시 데이터는 `runs/benchmarks/phase3/baseline-measurements.json`, `logged-measurements.json`과 인구별 요약 JSON에 보관했습니다. Phase 1의 기존 측정 파일은 덮어쓰지 않았습니다.

## 2026-11-09 — Phase 1 검증 기록

아래는 **2026-11-09 (Asia/Seoul)** 당시의 실제 실행 결과를 보존한 기록입니다. 이 절의 기능·성능·파일 해시는 Phase 1에 해당하며 현재 Phase 2·3 결과와 구분합니다.

### 환경

- Windows NT 10.0.26300.0, AMD64, 현재 프로세스에서 관측한 논리 프로세서 8개
- 프로세서 식별자: Intel64 Family 6 Model 189 Stepping 1, GenuineIntel
- Rust `1.98.1 (48a229cea 2026-09-01)`, Cargo `1.98.1 (797e8a9bc 2026-08-05)`
- `stable-x86_64-pc-windows-msvc`; Rust의 `rust-lld` + Microsoft CRT 14.44.35207 + Windows SDK 10.0.26100.0
- `bevy_ecs 0.19.1`, `rand 0.9.5`, `rand_chacha 0.9.0`; 나머지 버전은 Cargo.lock 참조
- 작업 폴더의 `.build-cache/vendor`를 통한 오프라인 빌드. 시스템 설치나 전역 Cargo 설정은 변경하지 않았습니다.

최초 Cargo 다운로드는 오프라인 캐시 부재와 제한된 네트워크 실행 경로 때문에 실패했고, MSVC `link.exe`도 없었습니다. 관리자 권한 확장은 허용되지 않았습니다. 기존에 허용된 직접 `curl.exe` 실행으로 공개 배포 파일을 작업 폴더 안에 받았고 crates.io/Microsoft 매니페스트의 SHA-256과 대조했습니다. 이후 모든 최종 빌드·테스트와 시뮬레이션은 오프라인으로 실행할 수 있습니다.

### 빌드와 자동화 테스트

다음 명령이 모두 종료 코드 **0**으로 완료되었습니다.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo test --workspace --offline --locked
cargo build --workspace --release --offline --locked
cargo run -p agentsociety-cli -- --agents 100 --ticks 1000 --seed 42
```

| 영역 | 테스트 수 | 검증 내용 |
| --- | ---: | --- |
| core | 4 | 예약 중복 사용 방지, 정산 보존, 오버플로 원자성, 제조 실패 원자성 |
| world | 2 | 초기화 시드 재현/변화, 환경 자원 부족 시 무변경 |
| economy | 9 | 무잔고/무재고 거절, 부분 체결, 가격/시간 우선, 미교차 만료, 자기 거래 방지, 제조 투입, 행동 검증 |
| cognition | 5 | 직업별 유효 의도, 목표별 식량 구매량, 무자산 자급, 잘못된 JSON, 수요에 따른 원자재 판매 |
| analytics | 2 | Gini 경계값/알려진 분포, 빈 사회의 유한 통계 |
| simulation 통합 | 8 | 동일 입력/이벤트 재현, 100명·1,000 Tick, 0/1/10/1,000명, 연속 sequence, 거절/계속 진행, 알 수 없는 ID 무변경, JSON 왕복, 여러 시드/화폐 0 |
| CLI 통합 | 4 | 실제 프로세스 파일 출력, 별도 프로세스 결정론, 설정/경로 오류, Tick 0/출력 덮어쓰기 |
| **합계** | **34** | **모두 통과, 실패/무시 0** |

실행 로그는 `runs/validation/tests.log`, `runs/validation/release-build.log`에 있습니다. 이 디렉터리는 생성 결과이므로 Git에서 제외합니다. Windows PowerShell 5의 stderr 병합 과정에서 컴파일 진행 문구가 `NativeCommandError` 형식으로 감싸진 부분이 있지만, 최종 Cargo 종료 코드와 테스트 결과는 위와 같습니다.

검증 중 발견해 수정한 실제 문제:

- Serde의 unit 행동 variant가 추가 JSON 필드를 허용하던 문제 → 빈 struct variant로 엄격한 필드 검증.
- 모든 Consumer가 가치가 높은 금속만 판매하여 목재 시장이 막히던 문제 → 직전 매수 수요와 부족량을 판매 우선순위에 반영, 회귀 테스트 추가.
- Gini가 JSON 왕복에서 마지막 비트가 달라지던 문제 → serde_json `float_roundtrip` 활성화, 정확한 왕복 테스트 유지.
- Clippy가 지적한 조건식 및 나눗셈 → let chain과 `checked_div`로 정리.

### 기본 실행: 100명, 1,000 Tick, seed 42

| 지표 | 결과 |
| --- | ---: |
| 전체 화폐 (초기 = 최종) | 972,137 |
| 누적 거래 체결 | 8,141건 |
| 누적 거래 대금 | 2,827,419 |
| 평균 평가 자산 | 10,885.53 |
| 최종 Gini | 0.8681 |
| 생성 이벤트 | 376,784개 |
| 거절 행동 | 0건 |
| 마지막 Tick의 기아 에이전트 | 0명 |

모든 금액은 정수 최소 단위입니다. 높은 최종 Gini는 이 단순한 생산/소비/저축 규칙에서 발생한 자산 집중이며 현실 경제 예측값이 아닙니다.

| 자원 | 초기 소유량 | 생산 | 생활 소비 | 제조 투입 | 최종 소유량 | 거래 수량 | 최종 가격 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Food | 1,375 | 99,483 | 100,000 | 0 | 858 | 13,285 | 84 |
| Wood | 450 | 6,948 | 0 | 7,196 | 202 | 3,280 | 54 |
| Metal | 275 | 3,386 | 0 | 3,598 | 63 | 1,260 | 126 |
| Goods | 225 | 4,631 | 4,795 | 0 | 61 | 2,149 | 418 |

각 행에서 `초기 + 생산 = 생활 소비 + 제조 투입 + 최종`이 성립합니다. 거래 수량은 소유권 이동이므로 이 식에 넣지 않습니다. 엔진은 이 검사를 매 Tick 수행합니다.

### 결정론과 출력 파일

Release 실행 두 번으로 100명·1,000 Tick·seed 42의 상태를 별도 파일에 기록했습니다. 한 실행은 이벤트/CSV 기록을 켰고 다른 실행은 끈 상태입니다. 두 최종 상태 파일의 SHA-256이 일치했습니다.

```text
9C74CD9B57E24CBD0B7BA809D264C433D4B9DD63085CF1A25261783DEA90DC4F
```

- `runs/final-state.json`, `runs/reproduced-state.json`: 동일한 결정론적 상태
- `runs/summary.json`: 설정, 통계, 누적 수지, 성능 측정
- `runs/events.jsonl`: 초기 상태 헤더와 모든 구조화 이벤트
- `runs/metrics.csv`: 1,000 Tick의 통계 행과 헤더

상태 JSON은 검사와 비교용이며 저장 복원/Replay가 구현되었다는 뜻은 아닙니다. 이벤트 단위 동일성은 120 Tick의 외부 입력 포함 통합 테스트에서도 확인합니다.

### Release 성능

조건은 seed 42, 1,000 Tick, 기본 이력 16개, 매 Tick 전체 보존 검사 활성화, JSONL 출력 해제입니다. `scripts/benchmark.ps1`로 실제 Release 실행 파일을 각각 한 번 실행했습니다. 컴파일 시간과 프로세스 시작/초기화 시간은 Rust의 `elapsed_ms`에 포함되지 않습니다.

| 에이전트 | Tick 루프 총 시간 | 평균 Tick | 최대 Tick | 평균 매칭·정산·만료 | 에이전트-Tick/초 | 관측 최대 작업집합 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 7.80 ms | 0.00755 ms | 0.110 ms | 0.00219 ms | 1,282,068 | 3.039 MiB |
| 100 | 125.29 ms | 0.12372 ms | 1.149 ms | 0.04180 ms | 798,126 | 5.617 MiB |
| 1,000 | 1,938.52 ms | 1.81139 ms | 5.534 ms | 0.51610 ms | 515,858 | 8.281 MiB |

메모리는 Windows 프로세스의 `PeakWorkingSet64`를 명목상 10ms 간격으로 읽어 관측한 값입니다. 실제 스케줄링 간격은 달라지며 각각 1/10/69개 표본입니다. 특히 10명 실행의 한 표본은 정확한 메모리 최대치라고 해석하지 않아야 합니다. 할당 추적기나 장시간 누수 검사는 아닙니다. 이 수치는 한 장비의 단회 측정이며 다른 환경의 성능 보장이 아닙니다.

100명 실행에서 JSONL을 켠 별도 측정은 총 **642.34 ms**, 직렬화/쓰기 구간 **492.54 ms**, 로그 **65,597,723 bytes**, 관측 최대 작업집합 **6.254 MiB**였습니다. 거래 건수와 최종 상태는 같았습니다. 이 조건에서는 이벤트 직렬화와 기록 구간이 전체 시간의 대부분을 차지합니다. 기본 버퍼링과 Tick 단위 방출을 사용하며, 전체 이벤트를 메모리에 누적하지 않습니다. 1,000명 기본 실행에서 시장 구간은 평균 Tick의 약 28.5%였습니다. 추가 최적화는 할당/CPU 프로파일링 후 진행합니다.

원시 결과는 `runs/benchmarks/baseline-measurements.json`, `logged-measurements.json`, 각 인구의 요약 JSON에 있습니다. LLM 추가 지연·비용과 GUI 반응성은 해당 기능이 미구현이므로 아직 측정하지 않았습니다.


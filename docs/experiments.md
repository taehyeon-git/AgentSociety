# 실험 비교와 성능 측정

`experiment`는 시나리오·인구·시드·정책의 모든 조합을 순서대로 실행합니다. 각 실행은 새 초기 상태와 새 정책 런타임을 사용합니다. 한 실행 안의 상태 변경은 단일 스레드이며 조합 간에도 병렬 실행하지 않습니다.

```sh
cargo run --release -p agentsociety-cli -- experiment --populations 10,100 --seeds 42,43,44 --policies rule-based --ticks 1000 --out-dir runs/experiments/rules --record-events
cargo run --release -p agentsociety-cli -- experiment --config examples/phase5.json --populations 10,100 --seeds 42,43,44 --ticks 1000 --out-dir runs/experiments/interactions
cargo run --release -p agentsociety-cli -- experiment --config examples/complete.json --scenarios examples/scenarios/crisis.json,examples/scenarios/redistribution.json --include-baseline --populations 100 --seeds 42,43,44 --ticks 1000 --out-dir runs/experiments/scenarios
cargo run --release -p agentsociety-cli --features llm -- experiment --populations 100 --seeds 42,43 --policies rule-based,hybrid,llm --llm-agents 4 --llm-config examples/llm-ollama.json --ticks 100 --out-dir runs/experiments/policies
```

기본값은 인구 100, 시드 42·43·44, 규칙 정책, 1,000 Tick입니다. `--config`의 기본 설정 위에 실험 인구·시드·정책을 적용합니다. 최대 1,000개 조합이며 모든 초기 설정과 공급자 설정을 검사한 뒤 결과 파일을 만듭니다. 실행 중인 모델의 이름·주소는 자신의 환경에 맞춰야 합니다.

`--scenarios`가 있으면 파일들의 시나리오를 교차하고, 없으면 기본 설정의 시나리오를 사용합니다. `--include-baseline`은 개입이 없는 동일 조건을 추가합니다. 같은 시나리오 이름을 중복 지정할 수 없습니다. 같은 체크포인트에서 미래만 바꾸는 대조는 [상태 분기](scenarios.md)의 `branch`를 사용합니다.

상호작용 기능별 비교는 `interactions`를 다르게 설정한 파일로 별도 실험 디렉터리에 실행합니다. 한 행렬 안에서 기능 조합 자체를 자동 순회하지는 않습니다. 벤치마크 스크립트의 `-Config`도 같은 설정을 전달하며 결과에 실제 상호작용 설정과 새 지표를 보관합니다. 스크립트의 `baseline`/`logged` 이름은 이벤트 기록 유무를 뜻합니다.

LLM 예산은 **조합마다** 새로 시작합니다. 예를 들어 총 호출 한도 100인 외부 정책 실험 6개라면 전체 예약 요청은 최대 600회입니다. 공급자가 보고한 토큰만 합산하며 요금이나 보고되지 않은 사용량은 추정하지 않습니다. 같은 시드를 다시 실행해도 모델의 새 출력까지 같다고 가정하지 않습니다. 정확한 재현은 저장된 JSONL의 `replay`로 수행합니다.

## 결과 파일

| 파일 | 내용 |
| --- | --- |
| `report.json` | 버전·완료 여부·요청 실행 수, 공급자 설정, 모든 결과, 그룹 비교 |
| `results.csv` | 한 실행당 한 행의 평탄한 지표 |
| `run-0001.summary.json` | 실제 초기 설정, 경제·자산 분포·사회관계·기억·사용량·시간·상태 해시 |
| `run-0001.checkpoint.json` | 해당 실행의 검증 가능한 최종 상태와 정책 사용량 |
| `run-0001.jsonl` | 전체 결정·이벤트; 외부 정책은 항상 기록, 규칙 정책은 `--record-events` 사용 시 기록 |

결과 보고서는 각 실행이 끝날 때 갱신하고 CSV도 flush합니다. 한 조합 이상 끝난 뒤 다음 조합에서 실패했다면 `completed: false`와 이미 끝난 실행을 확인할 수 있습니다. 첫 조합이 끝나기 전에는 새 보고서가 없을 수 있습니다. 실행 중인 조합 자체를 자동 복구하거나 실험 행렬의 남은 조합을 자동 재개하는 기능은 없습니다. 개별 완료 체크포인트는 `resume`할 수 있습니다.

기존 결과는 기본적으로 보호합니다. `--overwrite`는 이번 조합이 사용할 파일만 교체하며 이전 실행의 더 큰 행렬에서 남은 다른 파일을 삭제하지 않습니다. 현재 실행 목록은 `report.json`을 기준으로 읽습니다. 설정 파일과 출력 경로가 충돌하면 거절합니다.

## 지표 정의

- 개인 평가 자산은 현금·재고·공동 지분을 최종 체결 가격으로 평가한 합계입니다. 정부 금고는 개인 분포에서 제외합니다. 최소·최대, p10·중앙값·p90과 상위 10% 점유율을 제공합니다.
- 분위수는 정렬된 자산의 `(n - 1) × 비율` 위치를 선형 보간합니다. 상위 집단은 `ceil(n / 10)`명이며 빈 사회나 전체 자산 0의 점유율은 0입니다.
- Gini, 거래 체결 수·대금, 생산·생활 소비·제조 투입, 최종 기아와 평균 자산은 엔진의 실제 통계입니다.
- `negotiations`는 새 가격 제안 수, `negotiated_trades`는 실제 수락·정산된 협상 거래 수, `cooperations`는 완료 공동 생산 수, `goal_changes`는 실제 목표 변경 수입니다. 역제안은 새 협상 건수에 추가하지 않습니다. 협상 체결은 전체 거래 수·대금에도 포함됩니다. 각 실행의 누적값과 협상 체결·협력·목표 변경의 그룹 평균을 제공합니다.
- 사회 지표는 방향 있는 관계 수·평균 신뢰·연결 성분입니다. `communities`는 한쪽 이상의 신뢰가 20 이상인 관계를 무방향으로 투영한 그래프의 연결 성분 수로, 고립된 에이전트도 포함합니다. 별도의 군집 탐지 알고리즘 결과가 아닙니다.
- 시나리오·인구·정책별로 시드를 묶어 평균과 Gini 최소·최대 등을 비교합니다. 공동 생산·납품 이행/불이행·세금·재분배·식량 가격 평균도 포함합니다. 신뢰구간이나 통계적 유의성을 주장하는 검정은 제공하지 않습니다.
- `state_sha256`은 compact JSON의 결정론적 상태 해시입니다. 체크포인트 파일 전체나 시간 지표를 해시한 값이 아닙니다.

실험의 `elapsed_ms`는 Tick 루프와 그 안의 모델 대기·이벤트 기록을 포함하고 초기화·최종 체크포인트 저장·footer 작성은 제외합니다. `mean_tick_ms`는 엔진과 외부 결정 처리, `event_write_ms`는 JSONL 기록 구간을 측정합니다. 지표의 측정 범위가 다르므로 합계를 임의로 맞추지 않습니다.

## CPU·구간·메모리 측정

CLI 요약과 실험 결과의 `stages_mean_ms`는 환경, 결정, 행동, 시장, 생활 소비, 재정(`fiscal`), 사회관계, 목표 적응(`adaptation`), 통계·감사, 기억, 외부 요청의 Tick당 벽시계 시간입니다. 공동 제조는 행동, 납품 정산은 시장 구간에 포함합니다. 단계별 CPU 샘플링 수치가 아닙니다.

```powershell
cargo build -p agentsociety-cli --release --locked
.\scripts\benchmark.ps1 -Population 10,100,1000 -Ticks 1000 -Repetitions 3 -OutputDirectory runs\benchmarks\phase4
.\scripts\benchmark.ps1 -Population 100 -Ticks 1000 -Repetitions 3 -WithEvents -OutputDirectory runs\benchmarks\phase4
.\scripts\benchmark.ps1 -Config examples\phase5.json -Population 100,1000 -Ticks 1000 -Repetitions 3 -OutputDirectory runs\benchmarks\phase5
```

Windows 스크립트는 독립 Release 프로세스의 CPU 시간과 작업집합을 함께 기록합니다. `process_cpu_ms`에는 프로세스 시작과 초기화·종료도 포함됩니다. `sampled_peak_working_set_mib`는 명목상 10ms 간격으로 관찰한 작업집합이며 Rust의 정확한 힙 할당 크기가 아닙니다. 반복 횟수는 1~30, 기본 1이며 파일 이름은 반복 시 `baseline-1000-r1.json`처럼 구분합니다. 같은 출력 디렉터리의 같은 이름 측정은 갱신됩니다.

## 힙 할당 추적

일반 CLI와 GUI는 시스템 allocator를 유지합니다. 독립 예제만 [DHAT allocator](https://docs.rs/dhat/0.3.3/dhat/)로 빌드합니다.

```sh
cargo run -p agentsociety-cli --profile profiling --example heap_profile -- --agents 100 --ticks 50 --seed 42 --out runs/profiles/heap-100-50.json
```

`profiling` 프로필은 Release 최적화와 소스 위치 디버그 정보를 사용합니다. 추적 구간은 월드 초기화·Tick 실행·월드 해제이며 인자 파싱과 결과 직렬화는 제외합니다. stdout은 총 할당 횟수·누적 바이트·최대 동시 생존 힙·월드 보관량·해제 뒤 잔여량을 JSON으로 출력합니다. `--out`은 기존 파일을 교체하지 않고 호출 스택별 원시 추적을 저장합니다. [DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html)에서 추적 파일을 열어 분석할 수 있습니다.

Windows의 스택 수집은 실행을 크게 느리게 만들 수 있으므로 작은 실행부터 측정합니다. `profiled_elapsed_ms`는 추적 오버헤드를 포함하며 일반 Release 벤치마크와 비교할 처리 성능이 아닙니다. 누적 할당 바이트는 해제·재할당을 합한 양이고 동시에 필요한 메모리와 다릅니다. `--frames`는 수집 스택 깊이 제한이며 기본 16, 범위 4~64입니다.

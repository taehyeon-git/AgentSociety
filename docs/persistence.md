# 저장·복원·Replay

엔진 0.6.0은 상태/이벤트 포맷 6, 체크포인트 포맷 1, SQLite 스키마 1을 사용합니다. 저장한 엔진 버전과 포맷이 정확히 일치해야 복원·재현할 수 있습니다. 이전 파일의 자동 변환은 제공하지 않습니다. 작업 폴더의 이전 Release CLI는 `.build-cache/releases/0.4.0/agentsociety-cli.exe`와 `.build-cache/releases/0.5.0/agentsociety-cli.exe`에 보관했습니다.

포맷 6은 공동 계좌·회원·대표·해산 회계, 납품 계약의 잔여 회차·시각·예약, 시나리오 커서·활성 공급 충격·재정 정책·금고와 일반 이체 이력을 포함합니다. 복원은 개인/공동/정부 회계, 제안과 장기 계약의 합산 담보, 모든 계약 소유자와 미래 납품 시각, 일정에서 다시 계산한 현재 환경·정책을 대조합니다.

GUI의 `Load run...`과 공통 `read_run`은 JSON·SQLite·완료 JSONL을 읽기 전용으로 복원합니다. `Save checkpoint...`는 동일한 검증·원자적 저장을 사용합니다. [GUI 사용법](desktop.md)과 상태의 미래만 바꾸는 [branch 규칙](scenarios.md)을 참고합니다.

## 어떤 파일을 사용할지

| 목적 | 출력 / 입력 | 포함 내용 |
| --- | --- | --- |
| 최종 상태 비교, 규칙 실행 복원 | `--state-out state.json` | 설정, ECS와 독립적인 월드, 경제·관계·기억, 난수 위치, 다음 이벤트 번호 |
| 주기적 저장, 모든 정책의 실행 복원 | `--checkpoint-out checkpoint.json` | 엔진 상태 + 공급자 설정·누적 사용량·호출 간격 + SHA-256 |
| 전체 결정·이벤트 검증 | `--events events.jsonl` | 시작 상태, 이벤트, Tick별 정책 상태, 완료 체크포인트 |
| 강제 종료 후 복구, 장기 이력 조회 | `--database run.sqlite` | Tick 단위 확정 이벤트와 정책 상태, 주기적 체크포인트 |

실제 API 키는 어떤 형식에도 저장하지 않습니다. 공급자 설정에는 키를 읽을 환경변수 이름이 포함될 수 있습니다. 외부 정책의 일반 상태 파일은 요청 예산이 없으므로 `resume` 입력으로 거절합니다.

## CLI 사용

```sh
cargo run --release -p agentsociety-cli -- run --agents 100 --seed 42 --ticks 500 --checkpoint-every 100 --checkpoint-out runs/save/checkpoint.json --database runs/save/run.sqlite --events runs/save/first.jsonl
cargo run --release -p agentsociety-cli -- resume runs/save/checkpoint.json --ticks 500 --checkpoint-out runs/save/resumed-checkpoint.json --state-out runs/save/resumed-state.json --events runs/save/second.jsonl
cargo run --release -p agentsociety-cli -- replay runs/save/second.jsonl --state-out runs/save/replayed-state.json --json
```

두 번째 명령은 Tick 500에서 **500 Tick을 추가**하여 1,000에서 종료합니다. 이어 실행의 JSONL은 그 시작 상태를 포함하는 독립 구간이므로 앞 파일을 연결할 필요가 없습니다. 저장한 초기 설정·시드·인구·정책은 `resume`에서 바꿀 수 없습니다. 주기적 파일 체크포인트는 절대 Tick이 `--checkpoint-every`의 배수일 때 저장하며 정상 종료 시에도 저장합니다. 초기값은 100, 허용 범위는 1~1,000,000입니다.

```sh
cargo run --release -p agentsociety-cli -- resume runs/save/run.sqlite --ticks 500
cargo run --release -p agentsociety-cli -- replay runs/save/run.sqlite --checkpoint-out runs/save/verified.json --summary-out runs/save/replay-summary.json --json
cargo run --release -p agentsociety-cli -- inspect runs/save/run.sqlite --agent 0 --from-tick 1 --to-tick 1000 --limit 100 --json
```

SQLite는 원래 파일에 이어 기록합니다. 중단된 실행도 마지막 확정 Tick을 복구한 뒤 `--ticks`만큼 더 진행합니다. `--ticks 0`은 그 지점만 복구·검증하고 종료합니다. 이전 실행의 미완료 목표 Tick까지 자동으로 채우지 않습니다. 이어 실행 중 DB의 체크포인트 간격은 처음 생성한 값을 유지합니다. `--database`로 다른 DB를 동시에 지정하는 것은 허용하지 않습니다.

`inspect`의 Tick 범위는 양 끝을 포함하고 이벤트 sequence 순서로 반환합니다. `--agent`는 매수·매도 양쪽, 제안·협력 양쪽과 관계 갱신 당사자, 목표 변경 주체를 찾습니다. `--limit`은 1~10,000이며 기본 100입니다. 무결성 검사 뒤 색인을 사용해 조회하므로 아주 큰 이력의 조회 시간에는 검사 비용이 포함됩니다.

기존 출력 파일은 기본적으로 거절합니다. `--overwrite`는 지정한 일반 출력 파일에만 적용하며 입력 파일, 다른 출력과의 경로 충돌, SQLite 보조 파일 경로를 보호합니다. 기존 DB는 `--overwrite`로도 교체하지 않습니다. 같은 체크포인트 경로에 주기적으로 저장하는 경우 실행 자체가 만든 이전 버전을 교체합니다.

## 복원 검사

`Simulation::from_snapshot`은 설정과 초기 시드에서 기준 상태를 다시 만들고 다음을 확인합니다.

- 인구·ID·불변 신원, 지도 크기·위치·자원 풀, 정수 계좌와 진행 중 제안별 정확한 예약 합계
- 욕구·성격·기술 범위, 행동 이력의 연속성·보관 한도, 관계와 기억의 소유자·참조·순서
- 가격·주문 번호·이벤트 번호, ChaCha8 시드·스트림·진행 위치
- 에이전트별 초기 현금과 누적 수입·지출, 전체 화폐·자원 수지, 재계산한 통계
- 제안 역할·수량·가격·기한·역제안 한도·참여자 한도·중복 상대, 완료/진행 지표의 일관성
- 목표 검토·변경 Tick과 성과 구간, 초기 가격 기준 자산, 최신 변경 기억과 현재 목표

일반 주문장에는 완료 Tick의 미체결 주문이 남지 않습니다. 양자 제안은 다음 Tick까지 유지되므로 스냅샷의 `proposals`와 계좌 예약을 함께 저장·검사합니다. 단독 `World::from_snapshot`은 계좌 범위만 검사하며, 제안과의 대응은 `Simulation::from_snapshot`에서 검증합니다. 파일 체크포인트·SQLite 복구·JSONL Replay 모두 이 경계를 통과합니다.

`RunCheckpoint`는 추가로 공급자 설정과 LLM 사용량·마지막 요청 Tick을 검사합니다. Tick 간 기록에서도 예약 요청 수, 승인·대체 수와 호출 간격이 실제 외부 결과에 맞게 증가해야 합니다. `resume`은 예산을 초기화하지 않습니다. 새로운 LLM 호출이 필요하면 `--features llm`으로 빌드하고 환경변수 자격증명을 다시 제공합니다. 선택적으로 준 `--llm-config`도 저장된 값과 정확히 같아야 합니다.

체크포인트 파일은 같은 디렉터리의 임시 파일에 쓰고 동기화한 뒤 교체합니다. 검증 실패나 기존 파일 보호로 거절된 쓰기는 이전 파일을 유지합니다. 체크포인트·JSONL 한 문서의 읽기 한도는 256MiB이며 모든 과거 이벤트를 메모리에 누적하지 않습니다.

## JSONL 검증

각 줄의 `record_type`은 다음 네 종류입니다.

| 종류 | 역할 |
| --- | --- |
| `run_started` | 버전, 초기 상태, 초기 정책 상태, 이번 구간의 요청 Tick 수 |
| `event` | `EventRecord`의 sequence, Tick, 도메인 이벤트 |
| `tick_committed` | 해당 Tick의 이벤트 수, 완료 후 정책 상태 |
| `run_completed` | 최종 `RunCheckpoint`와 체크섬 |

Replay는 기록된 일반·외부 결정을 재사용해 엔진을 실행하고 **매 이벤트 전체 값**을 비교합니다. RNG도 정상 실행과 같은 횟수만큼 전진합니다. 마지막 상태·정책·체크섬까지 같아야 성공합니다. 서버나 API 키를 읽지 않으며 기본 빌드에서도 LLM 기록을 재현할 수 있습니다.

누락·중복 sequence, 틀린 Tick, 변경된 이벤트, 다른 최종 상태, 잘린 줄, 완료 footer 누락, footer 뒤 추가 데이터는 실패합니다. 실행 중 결정 불일치가 생긴 엔진은 실패 상태가 되어 계속 실행하거나 정상 상태로 내보낼 수 없습니다. JSONL의 중단 구간을 정상 완료로 간주하는 부분 복구 기능은 없습니다.

## SQLite 확정과 복구

한 DB에 한 실행을 저장합니다. WAL, `synchronous=FULL`, 외래키를 사용하며 `events`, `event_agents`, `tick_commits`, `checkpoints`, `metadata` 테이블을 관리합니다. Tick의 이벤트·참여자 색인·정책 사용량·현재 위치와 해당 Tick의 체크포인트를 하나의 트랜잭션으로 확정합니다. 쓰기 실패 시 전체 Tick이 롤백되고 이전 확정 위치가 남습니다. 오래된 위치를 가진 다른 writer의 추가 기록도 거절합니다.

`recover`는 헤더·이벤트 해시 체인·체크포인트 연결과 정책 사용량을 검사한 뒤 가장 최근 체크포인트에서 확정된 뒷부분을 재실행합니다. `replay`는 시작 상태부터 **모든** 확정 이벤트를 재실행하고 저장된 중간 체크포인트도 비교합니다. 중단 DB의 미확정 트랜잭션은 복구 대상에 들어가지 않습니다. 정상 종료된 프로세스의 DB를 복사할 때 사용하거나 SQLite의 백업 방법을 따르며, 실행 중인 DB를 보존할 때 WAL 파일을 임의로 버리지 않습니다.

SHA-256은 우발적인 데이터 손상을 검출하는 체크섬이며 서명이 아닙니다. `state_sha256`은 형식화된 파일 바이트가 아니라 타입을 복원한 뒤 직렬화한 compact JSON의 해시입니다. PowerShell `Get-FileHash` 등으로 얻는 pretty JSON 파일의 해시와 직접 비교하지 않습니다. 두 상태 파일 자체가 바이트 단위로 같은지는 별도로 비교할 수 있습니다.

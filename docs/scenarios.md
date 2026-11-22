# 공급 충격·위기·재정 정책과 분기 실험

`SimulationConfig.scenario`는 이름과 시간순 개입 목록입니다. 기본 `baseline`은 개입이 없습니다. 이름은 영문·숫자·`_`·`-` 최대 64자이며 한 스케줄은 최대 256개, 한 Tick은 최대 4개의 개입을 허용합니다. 라벨은 1~128자입니다. 초기화인 Tick 0에는 개입할 수 없습니다.

## 개입 종류

| `kind` | 실제 변화 | 경계 |
| --- | --- | --- |
| `supply` | Food/Wood/Metal의 환경 재충전량에 배율 적용 | 0~20000 bps, 지속 기간 1~1,000,000 Tick |
| `inventory_loss` | 개인·공동 계좌의 예약되지 않은 재고를 비율만큼 파괴 | 1~10000 bps, 정수 내림, 실제 손실 이벤트 |
| `fiscal` | 현재 판매세·현금 부담금·재분배 규칙 교체 | 이후 다른 변경까지 지속 |

10000 bps는 100%입니다. 공급 충격은 환경 자원 풀의 기본 상한·기본 재충전 필드를 바꾸지 않습니다. 시작 Tick부터 `시작 + duration - 1`까지 배율을 적용하고 다음 Tick에 정상으로 복귀합니다. 같은 자원의 공급 구간은 겹칠 수 없습니다. 가격 자체를 고정하지 않으며 실제 거래가 성사될 때 VWAP가 변합니다.

재고 손실은 예약 중인 현물 담보를 보호합니다. 미래 납품 계약의 재고는 미리 예약하지 않으므로 충격 때문에 부족해질 수 있습니다. 손실은 `destroyed`로 집계하며 생산 투입·생활 소비와 섞지 않습니다.

## 재정 규칙

```json
{
  "name": "tax_policy",
  "events": [{
    "tick": 20,
    "label": "Introduce funded redistribution",
    "intervention": {
      "kind": "fiscal",
      "policy": {
        "sales_tax_bps": 1000,
        "cash_levy_bps": 50,
        "redistribution_bps": 10000,
        "period": 10
      }
    }
  }]
}
```

판매세는 매 Tick 실제 중앙·협상·계약 판매액에 대해 부과합니다. 현금 부담금은 `tick % period == 0`인 시점에 판매세 납부 뒤의 가용 현금에 부과합니다. 개인과 조직 모두 과세하며 예약 현금은 사용할 수 없습니다. 납부 가능한 현금까지만 걷고 미납 세액을 부채로 만들지 않습니다.

판매세 최대 50%, 현금 부담금 최대 10%, 재분배 비율 최대 100%, 주기 1~1,000,000 Tick입니다. 세금은 정부 금고로 옮깁니다. 같은 주기의 과세 뒤 금고 현금의 설정 비율을 개인들에게 똑같이 지급하며 정수 나머지는 ID 순서로 한 단위씩 배분합니다. 인구가 없으면 지급하지 않습니다. 통화 발행·정부 적자·신용은 없습니다.

개입 시작·공급 복귀·재고 손실·실제 납세·실제 재분배는 이벤트로 남습니다. 체크포인트에는 스케줄 커서, 활성 공급 충격, 현재 재정 규칙, 금고와 이체 이력을 저장합니다. 복원은 완료 Tick까지의 스케줄에서 커서·규칙·충격을 다시 계산해 대조합니다.

## 새 실행과 같은 시드의 비교

```sh
cargo run --release -p agentsociety-cli -- run --config examples/complete.json --ticks 1000 --summary-out runs/complete/summary.json --csv runs/complete/ticks.csv
cargo run --release -p agentsociety-cli -- experiment --config examples/complete.json --scenarios examples/scenarios/crisis.json,examples/scenarios/redistribution.json --include-baseline --seeds 42,43,44 --populations 100 --ticks 1000 --out-dir runs/experiments/scenarios
```

`--scenario FILE`은 새 실행의 설정 파일에 있는 스케줄을 대체합니다. 실험의 `--scenarios`는 각 파일을 인구·정책·시드 조합과 교차합니다. 지정하지 않으면 기본 설정의 스케줄을 사용합니다. `--include-baseline`은 나머지 조건이 같고 개입만 없는 실행을 추가합니다. 이름 중복은 거절합니다.

`crisis.json`과 `redistribution.json`의 공급·재고 충격은 같습니다. 후자에는 재정 정책 변경을 더했습니다. 비교 그룹은 `(시나리오, 인구, 정책)`이며 같은 시드를 대응시켜 결과를 읽습니다. 가격·기아·Gini·평균 자산·세금·재분배·계약 이행/불이행을 비교할 수 있습니다. 이는 명시한 가상 경제 규칙의 결과입니다.

## 완료 상태에서 미래만 변경

```sh
cargo run --release -p agentsociety-cli -- run --config examples/complete.json --ticks 500 --checkpoint-out runs/complete/half.json --database runs/complete/source.sqlite
cargo run --release -p agentsociety-cli -- branch runs/complete/half.json --scenario examples/scenarios/branch-after-500.json --ticks 500 --checkpoint-out runs/complete/branch.json --events runs/complete/branch.jsonl --database runs/complete/branch.sqlite
cargo run --release -p agentsociety-cli -- replay runs/complete/branch.jsonl --json
```

`branch`는 JSON 체크포인트·상태, SQLite, 완료 JSONL을 읽어 독립 실행을 만듭니다. 원본 DB에 추가하지 않습니다. 개입은 모두 저장 Tick 뒤에 있어야 합니다. 과거 개입, 현재 정책·공급 충격, 모든 계좌·계약·기억, RNG 위치와 LLM 사용 예산을 유지하고 기존의 미래 일정만 새 목록으로 교체합니다. 활성 공급 충격과 새 구간이 겹치면 실패합니다.

`resume`은 기존 스케줄을 유지하므로 `--scenario`를 허용하지 않습니다. 빈 미래 목록을 가진 분기도 가능하며 현재 지속 정책은 그대로 유지됩니다. 정책을 해제하려면 미래 `fiscal` 개입에 0% 정책을 명시합니다. 분기 기록은 분기 시점부터 시작하며 원본의 전체 과거 로그를 복제하지 않습니다.

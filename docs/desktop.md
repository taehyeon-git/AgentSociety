# GUI 저장·복원과 시나리오

```sh
cargo run --release -p agentsociety-desktop -- --config examples/complete.json
```

상단 `Run`, `Pause`, `Step`, `Restart`, 속도·인구·시드는 완료 Tick 경계에서 worker가 적용합니다. `Social behavior`에서 협상·2인 제조·목표 검토·다자 조직 설정을 바꾼 뒤 `Restart`로 적용합니다. 저장된 실행을 불러오면 해당 설정·인구·시드와 지도 선택도 함께 갱신됩니다.

| 버튼 | 동작 |
| --- | --- |
| `Save checkpoint...` | 실행을 멈추고 현재 완료 상태와 정책 사용량을 `.json` 체크포인트로 저장 |
| `Load run...` | 체크포인트·규칙 상태 JSON, SQLite, 완료 JSONL을 검증한 뒤 일시정지 상태로 복원 |
| `Restart scenario...` | 다음 재시작에 쓸 시나리오 JSON을 선택; `Restart`에서 적용 |
| `Branch scenario...` | 현재 상태에서 미래 일정만 바꾼 독립 실행으로 분기; 새 파일에 저장하여 보관 |

파일 선택기에서 폴더를 클릭하거나 주소와 `Go`를 사용하고 `Up`으로 상위 폴더로 이동합니다. 파일 이름에는 절대 경로도 입력할 수 있습니다. 저장 대상이 이미 있으면 `Replace` 확인 화면을 표시하며 `Back`·`Cancel`은 기존 파일을 보존합니다. 저장은 같은 디렉터리의 임시 파일을 이용해 교체합니다.

파일을 선택하는 동안 실행은 일시정지합니다. 잘못된 파일, 다른 엔진 버전, 손상된 체크섬, 불일치하는 예약·회계 상태는 명시적인 오류로 표시합니다. 불러오기에 실패하면 현재 시뮬레이션을 유지합니다. 원본 SQLite는 읽기 전용으로 복원하며 GUI가 이후 Tick을 그 DB에 자동으로 추가하지 않습니다. 계속 진행한 상태는 `Save checkpoint...`로 저장합니다.

외부 정책은 체크포인트의 호출 한도·사용량·호출 간격을 유지합니다. 해당 빌드의 `llm` feature와 필요한 자격증명 환경변수가 있어야 계속 실행할 수 있습니다. GUI `Restart`는 새 실행이므로 예산을 초기화하지만 `Load`와 `Branch`는 초기화하지 않습니다.

Inspector에서 개인의 공동 지분·공동 재고·대표·회원과 납품 계약의 남은 횟수·기한·보증금을 확인합니다. Economy에는 공동 생산, 계약 이행/불이행, 정부 금고, 누적 세금·재분배와 공급 충격을 표시합니다. Timeline은 Organizations, Contracts, Scenario 필터를 제공하며 선택한 개인과 관련된 공동 계약도 조회합니다.

화면은 최근 통계 600개와 이벤트 1,500개를 유지합니다. 파일에서 복원하면 차트는 해당 완료 상태부터 다시 쌓고 과거 전체 로그를 화면으로 가져오지는 않습니다. 장기 이력은 CLI `inspect`·`replay`를 사용합니다.

## 검증 방법

`cargo test -p agentsociety-desktop --all-features`는 egui에 실제 포인터 누름·뗌과 키보드 이벤트를 전달하여 제어 버튼, 설정 재시작, 파일 선택·저장·교체 취소·불러오기·실패 보존·분기를 검사합니다. OS 창에 직접 마우스를 보내는 테스트와는 구분합니다. worker 테스트는 외부 정책의 예산 보존도 확인합니다.

네이티브 렌더러의 실행·진행·종료 검사는 다음과 같습니다.

```sh
cargo run --release -p agentsociety-desktop -- --config examples/complete.json --auto-start --exit-after-ticks 60
```

수동 점검 시에는 `Social behavior`의 설정을 바꾸고 `Restart` → `Step`을 실행한 뒤, 체크포인트 저장 → 추가 Step → 저장 파일 불러오기로 Tick이 복원되는지 확인합니다. 기존 파일 저장 시 `Cancel`이 파일을 보존하는지, 잘못된 파일 불러오기 후 기존 실행을 계속할 수 있는지도 확인합니다. 수행한 검증과 미확인 환경 범위는 [검증 기록](validation.md)에 남깁니다.

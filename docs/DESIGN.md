# AI Usage Monitor — 기술 설계 초안 v0.1

- 작성일: 2026-11-02
- 대상: Windows 11 x64 우선, Windows 10 호환은 후속 검증
- 대상 서비스: **Codex, Cursor 두 가지로 고정**
- 목표: 별도 앱을 열지 않고 Windows 작업 표시줄에서 잔여 사용량을 한눈에 확인
- 상태: 구현 전 설계 및 기술 검증 계획. 아래 수치 예시는 실제 계정 사용량이 아님.

구현 순서, 파일 단위 작업, 참고 코드에서 가져올 범위는 [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md)를 따른다. 런타임 스택은 그 계획의 구현 결정을 따른다.

## 1. 결론과 기본 전략

**`upstream-ray/codex-usage-monitor`를 MIT 라이선스 조건에 맞춰 포크하고, 기존 작업 표시줄 렌더링과 Win32 연동을 재사용한다.** Codex 수집부는 가능하면 설치된 CLI의 `codex app-server --stdio` RPC를 사용하는 방식으로 통일하고, Cursor는 별도 프로바이더로 추가한다. Cursor 수집부는 `PowerUserZ/OpenTokenUsage`의 참고 구현을 검토한다.

대안: `PowerUserZ/OpenTokenUsage`는 Codex와 Cursor 및 작업 표시줄 스트립을 이미 지원한다. 가장 빨리 작동하는 결과를 얻고 싶다면 이것을 포크하여 다른 프로바이더를 숨기는 방식도 가능하다. 다만 Tauri UI/플러그인 구조가 상대적으로 크고, 이 프로젝트의 이미지처럼 작은 2행 네이티브 작업 표시줄 UI에 맞추는 데 추가 변경이 필요할 수 있다.

### 참고 저장소

| 저장소 | 주요 참고 사항 | 출처 |
|---|---|---|
| upstream-ray/codex-usage-monitor | Windows 작업 표시줄 위젯, 드래그, DPI, 다중 모니터, 재시도 | https://github.com/upstream-ray/codex-usage-monitor |
| lch5518/CodexPeek | `codex app-server --stdio` 기반 인증/사용량 조회, 상태 관리 | https://github.com/lch5518/CodexPeek |
| PowerUserZ/OpenTokenUsage | Cursor 개인 계정 사용량 조회, 프로바이더 정규화, 작업 표시줄 스트립 | https://github.com/PowerUserZ/OpenTokenUsage |
| OpenTokenUsage Cursor 문서 | Cursor 내부 RPC의 요청/응답 형식 | https://github.com/PowerUserZ/OpenTokenUsage/blob/main/docs/providers/cursor.md |
| 공식 Codex app-server 문서 | `account/rateLimits/read` 응답 스키마 | https://learn.chatgpt.com/docs/app-server |

## 2. MVP 범위

### 포함

1. Windows 작업 표시줄 내부에 Codex + Cursor의 작은 사용량 영역 표시.
2. **Codex**: 서버가 실제 반환하는 primary/secondary 제한 창 사용량과 초기화 시각 표시. 기본 레이블은 5h/7d지만 기간은 응답 필드로 판단.
3. **Cursor**: 현재 결제 주기 `totalPercentUsed` 기준 잔여율, 결제 주기 종료 날짜 표시. Auto/API 별도 비율은 상세 툴팁에만 표시.
4. 5분 간격 자동 수집, 수동 갱신. 각 서비스는 독립적으로 갱신 및 오류 처리.
5. 최신 수집 실패 시 마지막 정상값 + 데이터가 오래되었다는 표시(`*` 또는 회색).
6. 시스템 트레이 메뉴(표시/숨기기, 새로고침, 윈도우 시작 시 실행, 5/10/15분 주기, 종료).
7. 각 서비스 활성화/비활성화, 단일 실행 인스턴스, Windows 탐색기 재시작 후 복구.
8. 계정 토큰과 원문 API 응답을 저장하거나 출력하지 않는 로그.

### 제외

다른 AI 서비스, 여러 계정 자동 전환, 사용량 예측, 통계 차트, 팀별 사용량, API 키 결제량 통합, 별도 클라우드 백엔드, 텔레메트리, 자동 업데이트 및 복잡한 커스터마이징.

## 3. 화면 구성

### 기본 배치

작업 표시줄 우측 트레이 왼쪽의 빈 공간에, 2열을 나란히 표시한다. 전체 너비 기본 360~420 DIP, 사용 가능한 공간이 부족하면 먼저 초기화 시간을 숨기고, 더 부족할 때는 한 줄 요약으로 자동 축소한다. UI는 Win32 작업 표시줄에 연결되는 컴팩트 네이티브 위젯이다.

```text
┌───────────────────────────┬──────────────────────────┐
│ Codex                     │ Cursor                   │
│ 5h  ██████░░░  64%        │ 월간 ███████░░  71%     │
│ 7d  ████████░  82%        │ 갱신 11/19              │
└───────────────────────────┴──────────────────────────┘
                     (Windows 작업 표시줄 트레이 왼쪽)
```

**표시되는 비율의 뜻을 ‘잔여율’로 고정한다.** 프로바이더가 사용률을 반환하면 `remainingPercent = clamp(100 - usedPercent, 0, 100)`으로 변환한다. 모든 상태와 툴팁에서도 `사용률`/`잔여율`을 혼동하지 않는다. 해당 서비스에서 실제로 제공하지 않는 항목은 `--`로 표시한다.

### 이벤트

- 마우스 오버: 사용률/잔여율, 갱신 시각, 다음 초기화 시각, 계정 상태를 표시(이메일은 표시하지 않아도 됨).
- 왼쪽 클릭: 선택적인 상세 소형 팝업(초기 MVP에서는 생략 가능).
- 오른쪽 클릭: 트레이 메뉴.
- 갱신 실패: 마지막 정상값을 유지하며 `*`/회색. 수치가 없는 초기 실패는 `--` 표시.
- 사용 가능한 작업 표시줄 공간이 부족하면 다른 Windows 아이콘/알림 영역을 덮지 않고 위젯을 숨기거나 축소.

## 4. 데이터 수집 설계

### CodexProvider (공식 CLI RPC)

1. 로컬의 `codex` 실행 파일을 탐색한다.
2. `codex app-server --stdio`를 자식 프로세스로 실행한다.
3. JSONL/JSON-RPC 세션을 초기화하고 `account/read`, `account/rateLimits/read`를 호출한다.
4. 우선 `rateLimitsByLimitId.codex`, 없으면 `rateLimits`를 사용한다. 응답이 제공하는 `primary`, `secondary`, `usedPercent`, `windowDurationMins`, `resetsAt`를 파싱한다.
5. 기간을 하드코딩하지 않는다. 응답에 창이 없다면 `--`로 표시한다. `resetsAt`은 Unix 초 단위다.
6. 사용량 확인을 위해 `codex exec` 또는 프롬프트를 실행하지 않는다. `auth.json`을 직접 파싱하지 않는다.

공식 참고: https://learn.chatgpt.com/docs/app-server

### CursorProvider (비공식 내부 인터페이스; 별도 검증 필요)

**중요:** Cursor는 개인 요금제 사용량의 공식 공개 API가 없다. 다음 방식은 커뮤니티 소스를 참고한 *역공학 내부 API*이므로 언제든 변경될 수 있다.

1. Windows Cursor 로그인의 로컬 SQLite 상태 DB(통상 `%APPDATA%\Cursor\User\globalStorage\state.vscdb`)를 **읽기 전용**으로 열어 `cursorAuth/accessToken` 존재 여부를 확인한다. 계정 로그인/버전에 따라 이 경로 및 저장 구조가 달라질 수 있다.
2. Bearer 토큰으로 `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage` 요청을 보낸다. `Content-Type: application/json`, `Connect-Protocol-Version: 1`, 본문 `{}`.
3. `planUsage.totalPercentUsed`와 `billingCycleEnd`를 우선 파싱한다. 비율이 없고 양의 한도가 있을 때만 `limit`/`remaining`으로 대체 계산한다. `autoPercentUsed`, `apiPercentUsed`, `spendLimitUsage`는 상세 정보용으로 분리한다.
4. `totalPercentUsed`가 없는 경우 **0%로 대체하지 말고 ‘확인 불가’**로 표시한다. 금액과 퍼센트의 단위를 섞지 않는다. 개인 계정 MVP만 보장 범위로 둔다.
5. Cursor 앱의 기존 데이터베이스를 수정하거나 브라우저 쿠키를 내보내지 않는다. 인증 만료나 401/403 발생 시 앱에서 재로그인하도록 안내하고, 마지막 정상값을 stale로 표시한다. MVP에서는 refresh token을 직접 저장/갱신하지 않는다.
6. Cursor 사용량 조회는 기능 사용자가 명시적으로 활성화해야 하며, 네트워크 송신 대상은 지정된 Cursor 도메인으로 제한한다.

참고: https://github.com/PowerUserZ/OpenTokenUsage/blob/main/docs/providers/cursor.md

## 5. 핵심 데이터 모델

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderId { Codex, Cursor }

#[derive(Clone, Debug)]
struct QuotaWindow {
    label: String,              // "5h", "7d", "월간"
    used_percent: Option<f32>, // None = 제공되지 않음
    remaining_percent: Option<f32>,
    resets_at: Option<i64>,    // UTC Unix seconds
}

#[derive(Clone, Debug)]
enum ConnectionStatus { Ok, Stale, LoginRequired, Unavailable, RateLimited }

#[derive(Clone, Debug)]
struct UsageSnapshot {
    provider: ProviderId,
    quotas: Vec<QuotaWindow>,
    status: ConnectionStatus,
    fetched_at: Option<i64>,
    error_category: Option<String>, // 민감한 원문 메시지 금지
}
```

비율 계산/표시는 공급자별 수집 코드에서 처리하고, UI는 `UsageSnapshot`만 소비한다. Cursor의 금액이나 Auto/API 지표는 추후 `details` 필드로 확장한다.

## 6. 모듈 구조

```text
ai-usage-monitor/
├─ Cargo.toml
├─ LICENSE
├─ THIRD_PARTY_NOTICES.md
├─ README.md
├─ docs/
│  └─ DESIGN.md
├─ src/
│  ├─ main.rs                # 단일 인스턴스, 앱 실행
│  ├─ app.rs                 # 초기화, 앱 상태
│  ├─ model.rs               # 표준화 모델
│  ├─ providers/
│  │  ├─ mod.rs              # Provider trait
│  │  ├─ codex.rs            # Codex app-server
│  │  └─ cursor.rs           # Cursor local auth + 내부 API
│  ├─ poller.rs              # 서비스별 스케줄/재시도
│  ├─ window.rs              # Win32 작업 표시줄 UI
│  ├─ native_interop.rs      # taskbar 탐색/연결/복원
│  ├─ tray.rs                # tray 메뉴
│  ├─ settings.rs            # %APPDATA%\AIUsageMonitor
│  └─ diagnostics.rs         # 민감정보 제거된 상태 로그
├─ tests/
│  ├─ codex_parser.rs
│  ├─ cursor_parser.rs
│  └─ poller.rs
└─ .github/workflows/
   └─ release.yml           # 추후 배포 단계
```

런타임은 **Rust + windows-rs + serde + ureq + rusqlite** 다. 작업 표시줄 코드가 동기 Win32 메시지 루프이므로 HTTP도 그 루프 밖의 동기 호출로 둔다. 배포와 작업 표시줄 연결은 `upstream-ray/codex-usage-monitor`의 동작 검증된 Win32 로직을 우선 재사용한다.

```text
Windows Explorer / Taskbar
         ▲
         │ (Win32 embed)
  TaskbarWidget ← UI State ← UsageNormalizer
                               ▲        ▲
                         CodexProvider  CursorProvider
                               ▲        ▲
                       codex app-server Cursor SQLite(Read-only)
                                              │
                                      Cursor 내부 사용량 RPC
```

## 7. 수집/갱신 정책

- 시작 즉시 두 서비스 병렬 조회 후 각자 갱신 주기 적용.
- 정상 주기 5분(사용자 설정 5/10/15분), 조회 중 같은 공급자 중복 호출 금지.
- 오류: 공급자별 2 → 5 → 10 → 15분 백오프; 서버의 `Retry-After`가 있으면 우선한다.
- 절전 복귀, 네트워크 복구 시 새로고침(디바운스).
- 마지막 정상값은 메모리에 보존. 시작 재표시용으로 디스크 저장 시 사용량 비율, 초기화 시각, 마지막 조회 시각만 저장; 절대 토큰/계정 ID/이메일 저장 금지.
- 타임스탬프는 UTC로 파싱 후 OS 로컬 시각으로 표시.

## 8. 보안 및 라이선스

1. Codex `auth.json` 직접 열람/복사/기록 금지. 기존 CLI 프로세스가 인증을 담당한다.
2. Cursor 상태 DB는 읽기 전용이고 선택적으로 활성화한다. 토큰, 쿠키, 원시 응답, 이메일, 계정 ID를 로그/설정에 남기지 않는다.
3. Cursor 내부 API는 공식 지원 대상이 아니며 호환성 위험을 README에 명시한다. 401/403 시 재인증 안내; 인증 우회/자동 브라우저 쿠키 추출 금지.
4. 소스 사용 시 MIT 원저작권 및 라이선스/서드파티 고지를 유지하고, 아이콘 등의 별도 라이선스를 확인한다.
5. 실행파일 배포 시 GitHub Actions 빌드 및 체크섬을 제공한다. 코드 서명이 없다면 그 사실을 명시한다.

## 9. 개발 단계 및 완료 기준

### 단계 0 — 기술 PoC (UI 개발 전에)

- `codex --version`, `codex login status` 확인.
- Codex app-server에서 1회 사용량 조회해 개인 계정 필드 확인(토큰/전체 응답은 기록하지 않음).
- Cursor가 로그인된 PC에서 SQLite 읽기 전용 토큰 조회 가능 여부와 내부 API 응답 필드 확인. 요청 실패 시 그 이유를 상태 코드로 남긴다.
- **게이트:** 두 데이터 소스 중 하나가 불가하면 해당 서비스는 mock으로 UI 개발을 계속하고, 실데이터 가능 여부는 명확히 미완료로 표시.

### 단계 1 — Codex 기반 작업 표시줄 위젯

- upstream-ray 포크 후 Claude/Antigravity UI 및 수집기 제거.
- 기존 Taskbar embedding/positioning, 트레이 메뉴, DPI/Explorer 재시작 복구 기능 보존.
- Codex 실데이터가 정확히 표시되는지 기존 Codex 앱/사용량 페이지와 대조.

### 단계 2 — Cursor 모듈 및 2열 UI

- Cursor 토큰 읽기(읽기 전용), 비공식 API 요청, 응답 파싱 및 에러 처리.
- `UsageSnapshot`으로 정규화 후 두 서비스를 독립 갱신.
- Cursor 갱신일과 사용량 대시보드 값 비교.

### 단계 3 — 품질/배포

- 단위 테스트: 값 없음, 0/100%, 잘못된 퍼센트, 429/401/403, 결제주기 전환, 빈 창, reset timestamp.
- Windows 실기 테스트: Explorer 재시작, 모니터 연결/해제, DPI 100/125/150%, Windows 라이트/다크, PC 절전/복귀, 작업표시줄 공간 부족.
- Portable exe + README 및 MIT 고지 생성. 최초에는 설치 프로그램/자동 업데이트 제외.

### MVP 승인 조건

- `Codex 5h/7d`와 `Cursor 월간` 값이 각 원본에서 제공될 때 사용자 확인 가능한 수준으로 일치.
- 한쪽 로그인 만료/네트워크 장애에도 다른 쪽 표시 정상.
- 작업 표시줄의 기존 버튼/알림 영역을 가리지 않고 Windows 탐색기 재시작 후 정상 복구.
- 민감정보가 로그/설정에 저장되지 않음.
- Windows 부팅 후 자동 실행 가능, 메인 창 없이 사용할 수 있음.

## 10. 다음 단계

가장 먼저 **Cursor 단독 PoC**를 구현해 개인 계정에서 내부 사용량 응답이 유효한지 확인하는 것이 중요하다. Codex와 UI는 기존 검증된 오픈소스를 활용할 수 있지만, Cursor API/인증이 프로젝트의 가장 큰 불확실성이다. PoC가 통과하면 upstream-ray 포크에 Cursor 프로바이더를 추가한다.

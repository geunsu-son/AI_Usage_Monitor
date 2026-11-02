# AI Usage Monitor 구현 계획

- 작성: 2026-11-02 (KST)
- 스펙: [DESIGN.md](DESIGN.md) v0.1
- 저장소 루트가 곧 앱 루트다. `Cargo.toml`은 `C:\dev\AI_Usage_Monitor`에 둔다.
- 이 문서는 다음 구현 세션이 그대로 따라갈 작업 순서다. 화면, 보안, 수집 필드의 기준은 설계 문서다. 런타임과 파일 편집 방법은 아래 구현 결정을 따른다.

## 한 줄 결론

`upstream-ray/codex-usage-monitor` `26f47f4`의 Win32 작업 표시줄 위젯을 이 저장소로 가져오고, Claude/Antigravity와 `auth.json` 직접 읽기를 제거한 뒤, Codex는 CodexPeek과 같은 `codex app-server --stdio` 세션으로, Cursor는 읽기 전용 SQLite + `GetCurrentPeriodUsage`로 붙인다.

작업 표시줄 코드를 새로 쓰지 않는다. `window.rs`는 약 5,100줄이고 임베드, DPI, 드래그, 다중 모니터, Explorer 재시작 복구가 이미 들어 있다.

## 구현 결정

확인한 기준 커밋:

| 출처 | 커밋 | 시각 (KST) | 쓰는 이유 |
|---|---|---|---|
| [upstream-ray/codex-usage-monitor](https://github.com/upstream-ray/codex-usage-monitor) | `26f47f422c70ab99e142ea5f567d1c12506db7c3` | 2026-10-09 11:22 | 작업 표시줄 위젯, 트레이, 폴링, 복구. 태그 버전 1.10.0 |
| [lch5518/CodexPeek](https://github.com/lch5518/CodexPeek) | `ff6e159a384271cf9ce5b57ddede3d1d4e7b35df` | 2026-09-17 17:59 | app-server JSONL 세션과 자식 프로세스 종료 |
| [poweruserz/OpenTokenUsage](https://github.com/poweruserz/OpenTokenUsage) | 문서만 참고 | — | Cursor 요청 형식. 코드는 가져오지 않는다 |

1. HTTP와 프로세스 실행은 동기 코드로 둔다. upstream은 `ureq` 2와 Win32 메시지 루프이고, CodexPeek도 동기 `ureq`다. Cursor POST도 `ureq`로 보낸다. `tokio`와 `reqwest`는 추가하지 않는다.
2. SQLite만 `rusqlite`에 `bundled` 기능을 켜서 추가한다. 시스템 sqlite 설치에 의존하지 않는다.
3. `windows` 크레이트는 upstream의 0.58을 유지한다. Job Object가 그 버전에서 컴파일되지 않을 때만 필요한 feature를 더하거나 0.61로 올린다.
4. 화면에 그리는 비율은 언어와 관계없이 잔여율이다. upstream은 간체 중국어에서만 잔여율을 보여주고 나머지 언어는 사용률을 그린다. `usage_percent_for_display`가 그 분기점이다.
5. 막대 채움도 잔여율과 같다. 잔여 64%면 막대의 약 64%가 채워진다.
6. Cursor는 기본 꺼짐이다. 트레이에서 켠 뒤에만 `state.vscdb`를 열고 `api2.cursor.sh`로 요청한다.
7. MVP는 Cursor refresh token을 호출하지 않고, Cursor DB에 쓰지 않는다. 액세스 토큰이 만료되면 `LoginRequired`와 재로그인 안내를 보여 준다. Cursor 앱이 켜져 있으면 앱이 DB의 토큰을 갱신하고, 다음 주기에 다시 읽는다.
8. Codex 로그인 갱신은 CLI에 맡긴다. `account/read`의 `refreshToken: true`는 인증 오류가 난 뒤 한 번만 보낸다. `auth.json`은 열지 않는다.
9. 기존 `codex-usage.exe`와 동시에 뜰 수 있게 뮤텍스, 창 클래스, 설정 폴더, 시작프로그램 값을 모두 `AIUsageMonitor`로 바꾼다.
10. 자동 업데이트, 언어/테마/알림 메뉴, 계정 전환, 사용량 예측, 일별 토큰 버킷은 MVP 메뉴와 호출 목록에 넣지 않는다. 그리기에 필요한 `Appearance` 기본값 코드는 남겨 둔다.

이 머신에는 2026-11-02 기준 `rustc`/`cargo`가 없다. 구현 첫 명령은 Rust 설치다.

## 가져올 것, 지울 것, 새로 쓸 것

upstream 트리를 저장소 루트에 복사한 뒤 아래대로 고친다. GitHub fork 네트워크에 이 저장소를 붙이지 않는다. MIT 고지와 핀한 커밋만 남긴다.

### 유지하고 고친다

| 파일 | 유지하는 동작 | 고칠 내용 |
|---|---|---|
| `src/native_interop.rs` | `find_taskbars`, `embed_in_taskbar`, DPI, 로컬 시간 변환, Explorer 위치 변경 훅 | 그대로 두고 호출부 이름만 맞춘다 |
| `src/window.rs` | 자식 창 임베드, 트레이 왼쪽 배치, 드래그, 다중 모니터, `spawn_taskbar_watchdog` | 프로바이더를 Codex/Cursor 두 개로 줄이고, 잔여율 표시와 공간 부족 축소를 넣는다 |
| `src/provider_poll.rs` | 서비스별 in-flight 금지, 백오프, 실패 시 마지막 값 유지 | 서비스 배열 길이를 3에서 2로 바꾼다. `auth.json` 변경 감시 제거 |
| `src/recovery_events.rs` | 절전 복귀, 네트워크 복구 후 디바운스 새로고침 | 유지 |
| `src/quota_text.rs` | 퍼센트 자리 고정 | 유지. 입력 문자열이 잔여율이 된다 |
| `src/theme.rs`, `src/tray_icon.rs`, `src/localization/` | 테마와 트레이 아이콘, 한국어/영어 문자열 | 새 메뉴 문구를 추가한다. 다른 언어는 영어 문자열로 폴백해도 된다 |
| `build.rs`, `src/icons/` | 실행 파일 아이콘 | 제품명을 AI Usage Monitor로 바꾼다. `src/icons/providers/`의 LobeHub MIT 고지를 유지한다. Claude 아이콘 파일은 삭제한다 |

### 삭제한다

- `src/updater.rs`와 업데이트 메뉴, winget 식별자
- Claude, Antigravity 수집 함수와 메뉴, `src/icons/providers/claude.*`
- `read_codex_credentials`, `codex_auth_path`, `CredReadW`로 Codex/Antigravity 자격 증명을 읽는 코드
- `fetch_codex_usage`의 Bearer HTTP 호출. upstream은 토큰을 읽어 `CODEX_USAGE_URL`에 직접 붙인다. 그 경로 전체를 제거한다
- 1분/1시간 주기 메뉴, 행별 표시 토글, 낮은 잔량 알림 메뉴, 언어 선택 메뉴, 외형 메뉴
- CodexPeek의 다중 프로필, 로그인 창, 일별 토큰, 자체 업데이트. 세션 코드만 참고하고 그 기능은 복사하지 않는다

### 새로 만든다

```text
src/model.rs
src/providers/mod.rs
src/providers/codex.rs
src/providers/cursor.rs
src/layout.rs
tests/codex_parser.rs
tests/cursor_parser.rs
tests/layout.rs
poc/src/main.rs          # 단계 0 전용. UI 전에 지운다
```

`window.rs`를 설계 문서의 `app.rs`/`tray.rs`/`settings.rs`로 미리 쪼개지 않는다. 두 프로바이더가 화면에 나온 뒤, 컴파일이 깨지지 않는 범위에서만 옮긴다. MVP 완료 조건에 파일 분리는 없다.

## 데이터 계약

`src/model.rs`에 설계 문서의 타입을 둔다. UI와 폴러는 이 타입만 주고받는다.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderId {
    Codex,
    Cursor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    pub label: String,
    pub used_percent: Option<f32>,
    pub remaining_percent: Option<f32>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Ok,
    Stale,
    LoginRequired,
    Unavailable,
    RateLimited,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSnapshot {
    pub provider: ProviderId,
    pub quotas: Vec<QuotaWindow>,
    pub status: ConnectionStatus,
    pub fetched_at: Option<i64>,
    pub error_category: Option<String>,
    pub details: ProviderDetails,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderDetails {
    pub auto_percent_used: Option<f32>,
    pub api_percent_used: Option<f32>,
}
```

공통 함수:

```rust
pub fn remaining_percent(used: f32) -> Option<f32> {
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    Some((100.0 - used).clamp(0.0, 100.0))
}
```

`used`가 없거나 0~100 밖이면 잔여율도 `None`이다. 0으로 바꾸지 않는다.

기간 라벨은 `windowDurationMins`로만 만든다.

| 분 | 라벨 |
|---|---|
| 없는 값 | `--` |
| 1440으로 나누어 떨어짐 | `{n}d` (10080 → `7d`) |
| 60으로 나누어 떨어짐 | `{n}h` (300 → `5h`) |
| 그 외 | `{n}m` |

창 객체는 있는데 기간만 없으면 퍼센트는 보여주고 라벨만 `--`다. primary와 secondary가 둘 다 없으면 두 행 모두 `--`이고 상태는 `Unavailable`이다.

Cursor 첫 행 라벨은 `월간`으로 고정한다. 둘째 행은 막대 없이 `갱신 {M}/{D}`다. 날짜는 `billingCycleEnd`를 UTC로 파싱한 뒤 OS 로컬 날짜로 찍는다. 종료 시각이 없으면 둘째 행은 `--`다.

실패 표시:

| 상황 | 화면 | 상태 |
|---|---|---|
| 이번 조회 성공 | 잔여율, 정상 색 | `Ok` |
| 예전 성공이 있고 이번 조회 실패 | 마지막 잔여율 + `*`, 회색 | `Stale` |
| 한 번도 성공 못 함 | `--` | `LoginRequired` / `Unavailable` / `RateLimited` |
| 퍼센트 필드 자체가 없음 | `--`, 툴팁 `확인 불가` | `Unavailable` |

`error_category`에 허용되는 값만 쓴다: `login_required`, `rate_limited`, `unavailable`, `timeout`, `cli_missing`. HTTP 본문, 예외 문자열, 경로, 이메일은 넣지 않는다.

디스크 캐시 `%APPDATA%\AIUsageMonitor\cache.json`에는 프로바이더, 라벨, 사용률, 잔여율, `resets_at`, `fetched_at`, `status`만 저장한다. 토큰, 이메일, 계정 ID, 원문 JSON은 필드 자체를 만들지 않는다.

## Codex 수집

### 프로세스

1. `where.exe`로 `codex.exe`, `codex.cmd`, `codex.ps1`를 찾고, `PATH`의 같은 파일명도 후보에 넣는다. CodexPeek `src/codex/locator.rs`의 후보 수집과 같다.
2. `codex --version` 출력에서 `codex`로 시작하는 버전을 파싱하고, 가장 높은 버전 하나를 고른다. 제한 시간은 5초.
3. 고른 실행 파일로 `app-server --stdio`를 띄운다. `CREATE_NO_WINDOW`(0x08000000). stdin/stdout 파이프. stderr는 버려도 되고 로그 파일에 붙이지 않는다.
4. 자식은 Job Object에 넣어, 타임아웃이나 부모 종료 때 프로세스 트리째 죽인다. CodexPeek `src/codex/process.rs`의 `ProcessGuard`를 이 범위만 가져온다.
5. 한 세션의 제한 시간은 15초. 같은 Codex 조회가 돌고 있으면 새 프로세스를 만들지 않는다.
6. `CODEX_HOME`을 바꾸지 않는다. 추가 프로필 디렉터리를 만들지 않는다.
7. `codex exec`, 프롬프트, `account/login/start`, `account/usage/read`는 호출하지 않는다.

### 와이어 형식

앱 서버는 줄 단위 JSON이며 `jsonrpc` 필드를 생략한다. 한 줄은 1 MiB를 넘으면 그 세션을 실패로 닫는다.

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"ai-usage-monitor","title":"AI Usage Monitor","version":"0.1.0"}}}
{"method":"initialized","params":{}}
{"id":2,"method":"account/read","params":{"refreshToken":false}}
{"id":3,"method":"account/rateLimits/read"}
```

`account/read` 결과의 `account`가 null이거나 `type`이 빈 문자열이면 `LoginRequired`다. `email`과 `id`는 serde에서 무시한다. 구조체에 필드를 만들지 않는다.

한도 응답은 다음 순서로 고른다.

1. `rateLimitsByLimitId.codex`
2. 그게 없으면 `rateLimits`

각 창에서 읽는 필드는 `usedPercent`, `windowDurationMins`, `resetsAt`뿐이다. `resetsAt`은 Unix 초다. 음수이거나 없으면 초기화 시각만 비운다.

`id` 3 호출이 전송 오류나 인증 오류면 프로세스를 새로 띄워 한 번만 재시도한다.

```json
{"id":2,"method":"account/read","params":{"refreshToken":true}}
{"id":3,"method":"account/rateLimits/read"}
```

재시도까지 실패하면 마지막 정상 스냅샷을 `Stale`로 둔다. 메서드가 없다는 오류(`-32601`)는 CLI가 오래됐다는 뜻으로 `Unavailable` + `cli_missing`이다.

파서 입력은 테스트에서 JSON 문자열로 넣고, 실제 자식 프로세스 없이 `cargo test`가 돌아야 한다. 세션 상태 기계는 `JsonlTransport` 트레이트로 감싸 CodexPeek 테스트와 같은 방식으로 스크립트 응답을 먹인다.

## Cursor 수집

개인 계정의 비공식 인터페이스다. README에 그 사실을 적는다. OpenTokenUsage 소스는 복사하지 않고, 공개된 요청 형식만 구현한다.

### 토큰 읽기

경로: `%APPDATA%\Cursor\User\globalStorage\state.vscdb`

```sql
SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'
```

- `rusqlite`를 `SQLITE_OPEN_READ_ONLY`로만 연다. `immutable=1`은 쓰지 않는다. WAL을 못 읽으면 켜져 있는 Cursor의 최신 토큰을 놓친다.
- DB 파일을 설정 디렉터리나 임시 폴더로 복사하지 않는다.
- `SQLITE_BUSY`면 그 주기만 `Unavailable` + `timeout`으로 보고 마지막 값을 유지한다.
- 키가 없으면 `LoginRequired`.
- 값은 요청 헤더에만 쓰고 함수가 끝나면 떨어뜨린다. `Debug`로 찍히지 않게 토큰 필드를 가진 구조체를 로그에 넘기지 않는다.
- `cursorAuth/refreshToken`, `cursorAuth/cachedEmail`은 SELECT 하지 않는다.

### 요청

Cursor가 트레이에서 켜져 있을 때만 보낸다.

```http
POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage
Authorization: Bearer <memory-only>
Content-Type: application/json
Connect-Protocol-Version: 1

{}
```

허용 호스트는 `api2.cursor.sh` 하나다. 리다이렉트를 따라가지 않는다. 제한 시간은 10초.

| 결과 | 상태 |
|---|---|
| 401, 403 | `LoginRequired`. 캐시의 마지막 숫자가 있으면 화면은 `Stale` |
| 429 | `RateLimited`. `Retry-After`가 초 또는 HTTP 날짜면 그 시각 전에 재시도하지 않는다 |
| 네트워크, 5xx, 시간 초과 | `Unavailable` |
| 200이지만 퍼센트를 만들 수 없음 | `Unavailable`, 화면 `--`. 0%로 채우지 않는다 |

### 퍼센트

1. `planUsage.totalPercentUsed`가 0 이상 100 이하의 유한 수면 그것이 사용률이다.
2. 그 필드가 없고 `planUsage.limit`이 양수이며 `planUsage.remaining`이 0 이상이면 사용률은 `(limit - remaining) / limit * 100`이다. `limit`과 `remaining`은 센트다. 이 값은 퍼센트로만 바꾸고, 금액은 화면과 툴팁에 넣지 않는다.
3. 둘 다 아니면 사용률은 `None`이다.

`billingCycleEnd`는 문자열이거나 숫자다. 절댓값이 10_000_000_000보다 크면 Unix 밀리초, 아니면 Unix 초다. 예시는 `"1771077734000"`처럼 밀리초 문자열이다. `resets_at`에는 초를 넣는다.

`autoPercentUsed`, `apiPercentUsed`는 `ProviderDetails`에만 넣고 툴팁에 `Auto 사용`, `API 사용`으로 적는다. 메인 막대에 쓰지 않는다. `spendLimitUsage`는 MVP 화면에 넣지 않는다.

팀 요금제, 브라우저 쿠키, OAuth refresh(`https://api2.cursor.sh/oauth/token`)는 MVP에 없다.

## 화면과 윈도우 코드

upstream `AppState`의 프로바이더 필드는 지금 세 벌이다.

- Claude: `session_percent`, `show_claude_code`
- Codex: `codex_session_percent`, `show_codex`
- Antigravity: `antigravity_session_percent`, `show_antigravity`

구현 시 이름 대응:

| 지금 | 바꾼 뒤 |
|---|---|
| `show_codex` | `show_codex` (기본 true) |
| `show_antigravity` | `show_cursor` (기본 false) |
| `show_claude_code`와 Claude 퍼센트 필드 | 삭제 |
| `provider_poll::Monitor.services` 길이 3, id 0/1/2 | 길이 2. id 0 = Codex, id 1 = Cursor |
| `poll_provider`의 Claude/Antigravity 가지 | 삭제 |

`usage_percent_for_display`는 항상 `remaining_percentage`를 반환하게 바꾼다. 간체 중국어 전용 분기를 제거한다. 막대에 넣는 값도 이 함수 결과를 쓴다.

Cursor 열은 Codex 열과 행 구성이 다르다. `format_codex_line`을 그대로 두 번 호출하지 말고, `layout.rs`의 순수 함수가 열 모델을 만들게 한다.

```rust
pub enum Row {
    Meter { label: String, remaining: Option<f32>, reset: Option<i64>, stale: bool },
    Text { label: String },
}

pub struct Column {
    pub title: &'static str,
    pub rows: [Row; 2],
}
```

Codex 열의 두 행은 `Meter`다. Cursor 첫 행은 `Meter { label: "월간" }`, 둘째 행은 `Text { label: "갱신 11/19" }`다. `window.rs`의 그리기 코드는 `Column`만 보고, 프로바이더 if를 더 늘리지 않는다.

툴팁 문장 예:

```text
Codex
5h 잔여 64% (사용 36%) · 초기화 21:10
7d 잔여 82% (사용 18%) · 초기화 11/09
마지막 성공 18:32

Cursor
월간 잔여 71% (사용 29%) · 결제 주기 종료 11/19
Auto 사용 4% · API 사용 46%
마지막 성공 18:32
```

이메일은 어떤 열에도 넣지 않는다. 실패 중이면 마지막에 `상태: 로그인 필요`처럼 범주만 덧붙인다.

### 공간이 부족할 때

배치 기준은 이미 있는 `tray_left - taskbar_rect.left`다. 위젯 너비가 그 값에서 8 DIP를 뺀 것보다 크면 아래 순서로 줄인다.

1. 전체: 라벨 + 막대 + 잔여율 + 초기화 시각
2. 컴팩트: 초기화 시각 제거
3. 한 줄: `Codex 64%   Cursor 71%`
4. 그래도 넘치면 위젯만 숨긴다. 트레이 아이콘과 메뉴는 유지한다. 작업 표시줄 아이콘이나 알림 영역 위로 이동하지 않는다.

이 선택은 `layout.rs`의 순수 함수로 만들고 픽셀 의존 테스트는 너비 숫자만 넣는다. `window.rs`의 `total_widget_width_for`는 그 결과를 배치에 쓰기만 한다.

왼쪽 클릭 팝업은 만들지 않는다. 오른쪽 클릭은 트레이 메뉴다. 호버는 위 툴팁이다.

### 트레이 메뉴

메뉴에 남는 항목:

1. 위젯 표시/숨기기
2. 새로고침
3. Codex 사용
4. Cursor 사용
5. 주기: 5분 / 10분 / 15분
6. Windows 시작 시 실행
7. 종료

`POLL_10_MIN = 600_000`을 추가한다. 저장값 검증은 5분, 10분, 15분만 허용하고 그 외는 5분으로 되돌린다. upstream 기본값과 1분/1시간 상수는 메뉴에서 빼는 김에 검증 범위도 맞춘다.

시작프로그램은 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`의 값 이름 `AIUsageMonitor`다. 명령은 현재 exe의 따옴표 감싼 경로다. upstream의 `CodexUsage` 값은 읽지도 지우지도 않는다.

단일 인스턴스 뮤텍스 이름은 `Local\AIUsageMonitor`다. 창 클래스도 `AIUsageMonitor`다.

설정 파일은 `%APPDATA%\AIUsageMonitor\settings.json`이다. 필드:

```json
{
  "widget_visible": true,
  "show_codex": true,
  "show_cursor": false,
  "poll_interval_ms": 300000,
  "tray_offset": 0,
  "anchor_left": false,
  "taskbar_index": 0,
  "monitor_device": null
}
```

`ClaudeCodeUsageMonitor` 설정 마이그레이션은 가져오지 않는다. 우리 앱의 이전 버전이 아직 없기 때문이다.

진단 로그는 `%APPDATA%\AIUsageMonitor\diagnostics.log`에 범주와 시각만 남긴다. 활성화 플래그가 꺼져 있으면 파일을 만들지 않는다.

## 폴링

- 프로세스가 뜬 직후 Codex와 Cursor를 각 스레드에서 한 번씩 조회한다. Cursor가 꺼져 있으면 Cursor 스레드는 요청을 만들지 않고 캐시도 읽지 않는다. 켜는 순간 즉시 한 번 조회한다.
- 정상 주기는 설정값(기본 5분)이다.
- 실패 백오프는 프로바이더마다 2분 → 5분 → 10분 → 15분이다. 15분에서 멈춘다.
- `Retry-After`가 있으면 그 대기 시간이 백오프보다 우선한다.
- 절전 복귀와 네트워크 복구는 upstream `recovery_events`의 디바운스를 그대로 쓰고, 복구 때 켜져 있는 프로바이더만 즉시 조회한다.
- 성공한 스냅샷만 메모리와 `cache.json`에 남긴다. 실패 응답으로 숫자를 덮어쓰지 않는다.
- 앱을 다시 열면 `cache.json`을 먼저 그려 `Stale`로 표시하고, 직후 조회가 성공하면 `Ok`로 바꾼다.

## 라이선스

구현 커밋에 아래를 포함한다.

- 루트 `LICENSE`: MIT. copyright 줄에 이 저장소 저작권과 함께 upstream이 이미 담고 있는 고지를 유지한다. upstream `LICENSE`는 Copyright (c) 2025 Craig Constable 이고, `Cargo.toml`의 winres 주석은 Code Zeno Pty Ltd와 Ray의 수정이다. 둘 다 `THIRD_PARTY_NOTICES.md`에 적는다.
- CodexPeek에서 가져오는 세션/프로세스 코드가 있으면 Copyright (c) lch5518 / CodexPeek MIT를 그 파일 헤더와 `THIRD_PARTY_NOTICES.md`에 적는다.
- `src/icons/providers/LICENSE`는 Copyright (c) 2023 LobeHub 다. OpenAI 마크를 Codex 열에 계속 쓰면 이 파일을 남긴다. 마크를 빼면 아이콘 파일도 함께 지운다.
- Cursor 아이콘을 새로 그리지 않는다. MVP 열 제목은 텍스트 `Codex`, `Cursor`면 충분하다.

코드 서명은 MVP에 없다. README에 서명되지 않은 portable exe라고 적는다. GitHub Actions 릴리스 워크플로는 동작하는 exe와 테스트가 생긴 다음 커밋으로 미룬다.

## 작업 순서

각 단계는 독립 커밋으로 끝내고, 그 시점의 `cargo test`가 통과해야 다음으로 간다. 단계 0이 실패해도 단계 1의 UI는 mock으로 진행하고, README에 어느 프로바이더가 미확인인지 적는다.

### 단계 0 — 도구와 PoC

1. `rustup`으로 stable `x86_64-pc-windows-msvc`를 설치한다. MSVC 빌드 도구가 없으면 그 사실을 README에 적고 설치를 안내한다. GNU 툴체인으로 바꾸지 않는다.
2. 저장소 루트에 임시 패키지 `poc`를 만든다. 의존성은 `serde`, `serde_json`, `ureq` 2, `rusqlite` bundled 뿐이다. Win32 UI를 넣지 않는다.
3. `poc`는 인자 `cursor` 또는 `codex` 하나로 한 번 조회하고 프로세스가 끝난다.
4. stdout에 허용되는 줄:

   ```text
   provider=cursor status=ok http=200 remaining_percent=71.0 resets_at=1762000000 auto=present api=present
   provider=codex status=ok windows=2 labels=5h,7d remaining=64.0,82.0
   ```

   토큰, 이메일, 원문 JSON, DB 경로는 출력하지 않는다. 실패하면 `status`와 `http` 또는 `error_category`만 출력한다.
5. 결과를 구현 노트에 남긴다. 숫자 자체는 커밋하지 않는다. 남길 내용은 `cursor=ok|login_required|unavailable`, `codex=ok|cli_missing|login_required` 같은 범주뿐이다.
6. PoC가 끝나 `poc/` 디렉터리를 삭제한다. 파서 테스트가 같은 규칙을 영구히 검증한다.

게이트: Cursor가 실패하면 `show_cursor` 기본값은 false인 채로 UI를 만들고, 실데이터 연동은 미완료로 표시한다. Codex CLI가 없으면 Codex 열은 fixture 스냅샷으로 그리고 `cli_missing` 툴팁을 붙인다.

### 단계 1 — upstream 트리를 올리고 두 열 껍데기로 줄이기

1. `26f47f422c70ab99e142ea5f567d1c12506db7c3`를 받아 `Cargo.toml`, `Cargo.lock`, `build.rs`, `src/`, `LICENSE`, `.gitignore`를 루트에 놓는다. `.git`은 가져오지 않는다.
2. 패키지 이름을 `ai-usage-monitor`, 바이너리 이름을 `ai-usage-monitor.exe`로 바꾼다. winres 제품명도 맞춘다.
3. 뮤텍스, 창 클래스, `SETTINGS_DIR`, 시작프로그램 값, 진단 파일 경로를 `AIUsageMonitor`로 바꾼다.
4. Claude와 Antigravity 수집, 메뉴, 아이콘, 상태 필드를 제거한다. 컴파일 오류가 가리키는 테스트만 고친다. 동작 중인 테스트의 단언을 약하게 바꾸지 않는다.
5. `updater.rs`와 업데이트 메뉴를 제거한다.
6. Antigravity 자리를 Cursor 목업으로 바꾼다. 목업 스냅샷은 `월간 잔여 71%`, `갱신 11/19`처럼 고정값이고 네트워크를 호출하지 않는다.
7. `usage_percent_for_display`를 잔여율 고정으로 바꾸고, 기존 중국어 전용 테스트를 모든 언어 기대로 수정한다.
8. `cargo test`와 `cargo build --release`가 통과해야 한다. 실행하면 메인 창 없이 작업 표시줄과 트레이만 보여야 한다.

이 단계의 Codex 숫자는 아직 목업이어도 된다. `auth.json`을 여는 함수가 트리에 남아 있으면 단계 1은 끝나지 않은 것이다.

### 단계 2 — Codex app-server

1. `src/providers/codex.rs`에 로케이터, 프로세스 가드, JSONL 세션, 파서를 넣는다.
2. `tests/codex_parser.rs`에 아래 픽스처를 먼저 넣고 파서를 맞춘다.
   - `rateLimitsByLimitId.codex`가 `rateLimits`보다 우선
   - primary만 있음, secondary만 있음, 둘 다 null
   - `windowDurationMins` 300 → `5h`, 10080 → `7d`, 90 → `90m`, 없음 → 라벨 `--`
   - `usedPercent` 0, 100, 100.4, -1, NaN
   - `resetsAt` 양수, 0, 음수, 없음
   - 알 수 없는 추가 필드가 있어도 파싱 성공. 그 필드를 구조체에 저장하지 않음
   - 1 MiB 이상 한 줄은 오류
   - 응답 id가 요청 id와 다르면 그 줄을 건너뛰고 다음 줄을 읽음
3. `poll_provider(0)`이 `providers::codex::fetch`만 호출하게 연결한다.
4. 로그인된 PC에서 Codex 사용량 화면과 위젯의 5h/7d 잔여율이 정수로 같아야 한다. 창 길이가 5h/7d가 아니면 화면에 실제 라벨이 보여야 하고, 테스트를 5h/7d 하드코딩으로 되돌리지 않는다.

### 단계 3 — Cursor와 실제 2열

1. `src/providers/cursor.rs`와 `tests/cursor_parser.rs`를 만든다.
2. 파서 픽스처:
   - `totalPercentUsed: 29` → 잔여 71, 라벨 `월간`
   - `totalPercentUsed` 없음, `limit: 40000`, `remaining: 16778` → 사용률 58.055, 잔여율은 clamp된 값
   - `totalPercentUsed` 없음, `limit: 0` → 퍼센트 `None`, 상태 `Unavailable`
   - `billingCycleEnd: "1771077734000"` → 초 단위 `resets_at`
   - 10자리 초 단위 숫자도 초로 해석
   - 401/403/429/500 매핑
   - `autoPercentUsed`, `apiPercentUsed`는 details에만 존재
3. 트레이에서 Cursor를 켜기 전에는 `state.vscdb` 열린 횟수와 HTTP 호출 횟수가 0인지 단위 테스트로 고정한다. 파일 시스템 대신 트레이트 대역을 쓴다.
4. `layout.rs`에 전체/컴팩트/한 줄/숨김 선택을 넣고 `tests/layout.rs`로 고정한다.
5. `window.rs`가 `Column`을 그리게 연결한다.
6. Cursor 대시보드의 기간 사용률과 위젯 잔여율이 정수로 맞는지, 종료일이 로컬 달력으로 맞는지 확인한다.

### 단계 4 — 상태, 설정, 보안 점검

1. 백오프, `Retry-After`, 단일 in-flight, 실패 시 마지막 값 유지를 `tests/poller.rs`로 옮긴다. upstream `provider_poll.rs` 안에 있는 테스트를 깨지 말고 프로바이더 수에 맞게 고친다.
2. `cache.json` 라운드트립 테스트에서 직렬화된 JSON에 `token`, `email`, `authorization`, `auth.json` 문자열이 없음을 assert한다.
3. 진단 로그 헬퍼 테스트도 같은 금지어를 검사한다.
4. README를 쓴다. 포함할 내용: 필요한 것(Windows 11 x64, 로그인된 Codex CLI, 선택적 Cursor 로그인), 실행 방법, Cursor API가 비공식이라는 문장, 서명되지 않은 exe라는 문장, 저장하지 않는 데이터, MIT 고지 위치.
5. `.github/workflows/release.yml`은 이 단계에 넣지 않는다.

## 실기에서 볼 것

자동 테스트로 대체하지 못하는 항목이다. 구현 세션이 직접 확인하고, 못 한 항목은 PR에 적는다.

- Explorer를 재시작하면 위젯이 트레이 왼쪽으로 돌아오고 프로세스가 둘 이상 남지 않는다.
- 주 모니터와 보조 모니터 작업 표시줄 사이를 옮길 수 있다.
- DPI 100%, 125%, 150%에서 글자가 잘리지 않는다.
- Windows 라이트/다크에서 막대와 글자가 보인다.
- 절전에서 깨어나면 한 번 새로고침된다.
- 작업 표시줄에 아이콘이 많아 너비가 부족하면 초기화 시각이 먼저 사라지고, 더 부족하면 한 줄이 되며, 그래도 부족하면 위젯만 숨는다. 알림 영역 아이콘이 가려지지 않는다.
- Codex만 로그인 만료여도 Cursor 숫자가 남는다. 반대도 같다.
- Cursor를 끄면 해당 열이 사라지고, 그 뒤 네트워크 모니터에 `api2.cursor.sh` 요청이 더 이상 없다.
- 설정 폴더의 파일을 텍스트 검색하면 토큰과 이메일이 없다.

## 완료 조건

- Codex 창과 Cursor 월간 잔여율이 각 원본 화면과 정수 기준으로 같다.
- 한 서비스의 로그인 만료나 네트워크 오류가 다른 서비스 숫자를 지우지 않는다.
- 작업 표시줄의 기존 버튼과 알림 영역을 가리지 않고, Explorer 재시작 후 위젯이 돌아온다.
- 로그, 설정, 캐시에 토큰, 이메일, 계정 ID, 원문 응답이 없다.
- 시작프로그램으로 로그온 뒤 메인 창 없이 위젯과 트레이만 뜬다.
- `cargo test`가 통과한다.

## 구현 세션이 하지 않을 일

- `window.rs`를 설계 문서의 모듈 그림대로 먼저 분해하지 않는다.
- OpenTokenUsage나 CodexPeek 저장소 전체를 벤더링하지 않는다.
- Cursor DB를 쓰거나 refresh token을 저장하지 않는다.
- `auth.json`을 읽거나 복사하지 않는다.
- 사용량 확인용으로 Codex 프롬프트를 실행하지 않는다.
- 자동 업데이트, 설치 프로그램, 다른 AI 서비스, 계정 전환, 예측 그래프를 넣지 않는다.

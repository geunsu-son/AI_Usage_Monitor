# AI Usage Monitor

**한국어** | [English](README.en.md)

![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Windows 작업 표시줄에서 Codex·Cursor의 **잔여** 사용량(세션·주간 창)을 보여주는 위젯입니다.

![AI Usage Monitor running in the Windows taskbar](.github/animation.gif)

## 참고한 저장소

이 프로젝트는 아래 오픈소스의 설계·코드를 참고해 만들었습니다. 상세 고지와 라이선스는 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)를 보세요.

| 저장소 | 참고 범위 |
|---|---|
| [upstream-ray/codex-usage-monitor](https://github.com/upstream-ray/codex-usage-monitor) | Win32 작업 표시줄 위젯, 트레이, 폴링, 복구 로직 (기반) |
| [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor) | upstream-ray의 원천 계보 (메타데이터 고지) |
| [lch5518/CodexPeek](https://github.com/lch5518/CodexPeek) | Codex `app-server --stdio` 세션 처리 |
| [poweruserz/OpenTokenUsage](https://github.com/poweruserz/OpenTokenUsage) | Cursor 요청 형식 문서 (코드 미복사) |

GitHub fork 네트워크에 묶이지 않은 독립 저장소이며, MIT 조건에 맞춰 저작권·허가문을 남깁니다.

UI 언어는 **한국어·영어**만 지원합니다.

## 빌드

[Rust](https://rustup.rs/)(MSVC 툴체인)와 C++ 워크로드가 포함된 Visual Studio Build Tools가 필요합니다.

```powershell
cargo build --release
```

실행 파일은 `target\release\ai-usage-monitor.exe`입니다.

### 프로바이더 검증 (GUI 없음)

```powershell
cargo build
.\target\debug\ai-usage-monitor.exe --probe all
```

결과는 `%TEMP%\ai-usage-probe.txt`에 추가됩니다. Codex는 OpenAI Codex CLI가 설치되어 있어야 하고(보통 `%LOCALAPPDATA%\OpenAI\Codex\bin\codex.exe`), Cursor는 데스크톱 앱에 로그인되어 있어야 합니다.

## 사용법

- **Codex**: [CodexPeek](https://github.com/lch5518/CodexPeek)과 같은 `codex app-server --stdio` 방식을 사용합니다. 필요하면 Codex CLI로 로그인하세요.
- **Cursor**: 로그인되어 있으면 기본으로 켜집니다. 트레이 **Models** 메뉴에서 끌 수 있습니다. `%APPDATA%\Cursor\User\globalStorage\state.vscdb`를 읽기 전용으로 읽고 `api2.cursor.sh`를 호출합니다. 위젯에는 **Auto**(포함 모델)와 **API**(외부 모델) 한도가 표시되며, Cursor만 켠 경우 행 라벨은 로케일을 따릅니다(한국어면 자체 모델 / 외부 모델).

설정·뮤텍스 이름은 `AIUsageMonitor` 접두사를 쓰므로, upstream의 Codex 전용 모니터와 나란히 실행할 수 있습니다.

## 문서

- [DESIGN.md](docs/DESIGN.md)
- [IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md)
- [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)

## 라이선스

MIT — [LICENSE](LICENSE) 참고.

<div align="center">

# GpuTerm

**GPU 서버를 위한 올인원 SSH/SFTP 데스크톱 클라이언트.**

터미널, 파일 전송, 그리고 CPU · RAM · 디스크 · NVIDIA GPU 실시간 모니터링 — 하나의 네이티브 창에서.

[![Release](https://img.shields.io/github/v/release/fortranmentis/GPUTERM?include_prereleases&label=release&color=2ea44f)](https://github.com/fortranmentis/GPUTERM/releases)
[![License: MIT](https://img.shields.io/github/license/fortranmentis/GPUTERM?color=blue)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-8b5cf6)](#설치)
[![Built with Tauri](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=white)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

[English](./README.md) · [한국어](./README.ko.md)

<img src="docs/screenshots/main.png" alt="GpuTerm 메인 창: 점프 호스트를 지원하는 세션 사이드바, SSH 터미널, SFTP 브라우저, 텔레메트리 바" width="850" />

</div>

---

원격 GPU 서버에서 작업하다 보면 SSH 클라이언트, SFTP 도구, 그리고 `watch nvidia-smi`를 띄워둔 터미널까지 세 개의 창을 오가게 됩니다. **GpuTerm은 이 셋을 하나로 합쳤습니다.** 한 번 접속하면 xterm.js 터미널, 드래그 앤 드롭 SFTP 브라우저, 그리고 CPU·메모리·디스크·로그인 사용자·NVIDIA GPU 전체를 폴링하는 실시간 텔레메트리 바가 함께 열립니다. 모니터링은 별도 SSH 채널로 동작하므로 셸 작업을 방해하지 않습니다.

> **상태:** 베타. 최신 프리릴리스는 [Releases](https://github.com/fortranmentis/GPUTERM/releases)에서 내려받거나 아래 안내에 따라 소스에서 빌드하세요.

## 주요 기능

### 🖥️ SSH 터미널
- [xterm.js](https://xtermjs.org)와 Rust [`ssh2`](https://crates.io/crates/ssh2) 기반의 완전한 PTY 터미널
- **다중 세션 동시 접속** — 세션마다 터미널·스크롤백·SFTP 경로를 독립적으로 유지하며, 사이드바에서 연결된 프로필을 클릭하면 전환
- **ProxyJump** — 저장된 프로필을 점프 호스트(bastion)로 지정해 경유 접속 (경유 구간마다 키 종류별 호스트 키 검증)
- 비밀번호, 개인키(패스프레이즈 포함), SSH 에이전트 인증 지원
- UTF-8 안전 스트리밍 — 청크 경계에 걸린 멀티바이트 문자(한글, 日本語, 이모지)가 깨지지 않음
- MOTD를 포함한 접속 초기 출력을 버퍼링 후 재생 — 연결 타이밍에 유실되지 않음
- 원격 PTY 크기 자동 동기화 및 SSH keepalive

### 📁 SFTP 브라우저
- 원격/로컬 패널을 나란히 두고 드래그 앤 드롭으로 업로드·다운로드
- 1 MiB 청크 스트리밍 전송, 진행률 큐, **파일별 전송 취소**
- 다운로드는 임시 파일에 쓴 뒤 원자적으로 교체 — 불완전한 파일이 남지 않음
- 덮어쓰기 확인, 삭제, 폴더 생성, OS 네이티브 폴더 선택기
- 터미널/SFTP 창 사이 너비 조절 스플리터 (재시작 후에도 유지)

### 📊 실시간 텔레메트리
- 하단 상태바에서 CPU, RAM, 디스크, 로그인 사용자, **NVIDIA·AMD(ROCm)·Intel** GPU를 1~10초 주기로 폴링
- 각 섹션을 클릭하면 **드래그·크기 조절이 가능한 상세 팝오버**: 코어별 CPU 사용률, 상위 프로세스, GPU별 VRAM/전력/온도, 전체 마운트 목록
- **상세창을 별도 OS 창으로 분리** 가능 — 독립적으로 갱신되고 세션이 끊기면 함께 닫힘
- 텔레메트리는 세션별 전용 SSH 연결에서 동작하며 끊기면 지수 백오프로 자동 재연결
- NVIDIA GPU가 없는 서버에서는 시스템 지표만 표시하며 정상 동작

### 🔐 기본 보안
- 비밀번호는 메모리에서만 유지 — 디스크에 기록하지 않음
- 최초 접속 시 SHA-256 호스트 키 지문을 확인하는 TOFU 프롬프트, 지문 불일치 시 연결 차단
- 프로덕션·개발 창 모두에 제한적인 Tauri Content Security Policy 적용

## 설치

### 빌드된 설치 파일

[최신 릴리스](https://github.com/fortranmentis/GPUTERM/releases)에서 OS별 설치 파일(`.msi`/`.exe`, `.dmg`, `.deb`/`.AppImage`)을 내려받으세요.

### 소스에서 빌드

**필수 구성 요소:** [Node.js](https://nodejs.org) ≥ 20, npm ≥ 10, [Rust](https://rustup.rs) stable, OS별 [Tauri 필수 구성 요소](https://tauri.app/start/prerequisites/).

<details>
<summary>OS별 필수 구성 요소 상세</summary>

**Windows**
- Visual Studio Build Tools 2022 (*Desktop development with C++* 워크로드)
- WebView2 Runtime (Windows 10/11에는 기본 포함)
- Git for Windows
- [Strawberry Perl](https://strawberryperl.com) (`winget install StrawberryPerl.StrawberryPerl`) — SSH 라이브러리가 사용하는 내장 OpenSSL 컴파일에 필요

**macOS**
```bash
xcode-select --install
```

**Linux (Debian/Ubuntu)**
```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

</details>

```bash
git clone https://github.com/fortranmentis/GPUTERM.git
cd GPUTERM
npm install

# 개발 모드로 데스크톱 앱 실행
npm run tauri:dev

# 배포용 패키지 빌드 (출력: src-tauri/target/release/bundle)
npm run tauri:build
```

> `npm run dev`는 Vite 프론트엔드만 실행합니다 — 레이아웃 작업에는 유용하지만 SSH/SFTP/텔레메트리는 Tauri 앱 전체가 필요합니다.

## 사용법

1. **프로필 생성** — 사이드바에 host, port, username과 비밀번호 또는 개인키 경로를 입력합니다. **New**로 새 프로필을 시작하고 **Save**로 저장합니다.
2. **접속** — 최초 접속 시 서버의 SHA-256 호스트 키 지문을 보여주고 신뢰 여부를 확인받습니다. 여러 서버에 동시에 접속할 수 있으며, 연결된 프로필에는 초록 점이 표시되고 클릭하면 해당 세션으로 화면이 전환됩니다.
3. **작업** — 터미널에 입력하고, SFTP 패널 사이로 파일을 끌어다 놓고, 하단 바에서 실시간 지표를 확인하세요. CPU / RAM / Disk / GPU / Users를 클릭하면 상세 팝오버가 열리며, 드래그·크기 조절은 물론 ↗ 버튼으로 별도 창 분리도 가능합니다.

<details>
<summary>SFTP 전송 상세</summary>

- 여러 파일을 한 번에 드롭할 수 있으며, 각 파일은 진행률·상태·오류를 개별 보고하는 큐 항목이 됩니다.
- 진행 중인 전송은 큐에서 개별적으로 취소할 수 있습니다.
- 대상 파일이 이미 있으면 덮어쓰기 전에 확인을 요청합니다.
- 마지막 로컬 디렉토리는 다음 실행 시 복원됩니다.
- 디렉토리 드래그 앤 드롭은 감지되지만 아직 전송되지 않습니다 ([로드맵](#로드맵--알려진-제한) 참고).

</details>

<details>
<summary>텔레메트리 설정</summary>

- **Interval:** 1, 2(기본), 5, 10초 — 상세 팝오버도 같은 주기로 폴링합니다.
- **Mode:** GPU + System, GPU only, System only.
- **Ignore FS:** 디스크 요약에서 숨길 파일시스템 타입을 쉼표로 구분해 지정 (기본: `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, `overlay`). 디스크 팝오버에서 일시적으로 표시할 수 있습니다.
- 마운트 우선순위는 `/` → `/home` → `/data` → `/mnt*` → `/media*` → 기타이며, 사용률 80% 이상은 경고, 90% 이상은 위험으로 표시됩니다.

</details>

<details>
<summary>텔레메트리가 실행하는 원격 명령</summary>

모든 지표는 표준 도구를 SSH로 실행해 수집합니다 — 서버에 아무것도 설치하지 않습니다.

| 섹션 | 명령 |
| --- | --- |
| CPU | `/proc/stat`, `/proc/loadavg`, `/proc/cpuinfo`, `nproc`, `lscpu` |
| 메모리 | `/proc/meminfo` |
| 디스크 | `df -P -T -B1` |
| 사용자 | `who` |
| GPU | `nvidia-smi`(NVIDIA), `rocm-smi --json`(AMD/ROCm), `xpu-smi` / `intel_gpu_top`(Intel) — 호스트별 자동 감지 |
| 상위 프로세스 | `ps -eo … --sort=-%cpu` / `--sort=-rss` |

명령은 전용 SSH 연결에서 3초 타임아웃으로 실행됩니다. GpuTerm이 호스트별로 설치된 GPU 도구를 감지해 각 카드에 벤더 태그를 표시하며, `intel_gpu_top`은 root 또는 `CAP_PERFMON` 권한이 필요합니다. GPU 도구가 하나도 없으면 GPU 섹션만 '사용 불가'로 표시되고 나머지는 계속 동작합니다.

</details>

## 아키텍처

```
┌───────────────────────────── Tauri window ─────────────────────────────┐
│  React 19 + TypeScript + Zustand + xterm.js                            │
│    invoke() ──────────────► Tauri commands (Rust)                      │
│    listen() ◄────────────── terminal-output · remote-telemetry ·       │
│                             sftp-progress · terminal-closed            │
├────────────────────────────────────────────────────────────────────────┤
│  Rust backend (ssh2 / libssh2)                                         │
│    • 터미널        – 세션당 전용 연결의 PTY 셸                          │
│    • 텔레메트리    – 자체 연결, 백오프 기반 자동 재연결                 │
│    • SFTP 작업     – 세션별로 재사용하는 "작업용" 연결 풀               │
│    • 대용량 전송   – 파일별 전용 연결, 취소 가능                        │
└────────────────────────────────────────────────────────────────────────┘
```

무거운 작업은 격리되어 있습니다: 블로킹 SSH I/O는 `spawn_blocking` 스레드에서 실행되어 UI가 멈추지 않으며, 셸·텔레메트리·전송이 각각 독립적으로 실패합니다.

**데이터 저장 위치** (Windows: `%APPDATA%\GpuTerm`, macOS: `~/Library/Application Support/GpuTerm`, Linux: `~/.config/GpuTerm`):

| 파일 | 내용 |
| --- | --- |
| `sessions.json` | 세션 프로필 — host, port, username, 키 *경로*만 저장 |
| `known_hosts.json` | 승인된 SHA-256 호스트 키 지문 |
| `app_settings.json` | 마지막 로컬 SFTP 디렉토리 등 UI 설정 |

비밀번호와 개인키 내용은 어떤 파일에도 **절대** 기록되지 않습니다.

## 개발

```bash
npm run test                                   # 프론트엔드 테스트 (Vitest)
cargo test --manifest-path src-tauri/Cargo.toml # 백엔드 테스트
npm run build                                  # TypeScript + Vite 프로덕션 빌드
```

<details>
<summary>프로젝트 구조</summary>

```
src/                    React 프론트엔드
  components/           TerminalPane, SftpBrowser, RemoteTelemetryBar, 팝오버…
  stores/               Zustand 스토어 (세션, 전송)
  utils/                공용 포맷터, 터미널 버퍼, 디스크 우선순위
src-tauri/src/ssh/      Rust 백엔드
  terminal.rs           PTY 셸 + UTF-8 안전 리더
  system_monitor.rs     텔레메트리 루프 + 파서
  resource_details.rs   CPU/RAM/GPU 상세 지표 수집
  sftp.rs               전송, 취소, SFTP 명령
  session.rs            연결, 호스트 키, 프로필, 연결 풀
```

</details>

## 문제 해결

| 증상 | 확인 사항 |
| --- | --- |
| Windows에서 `tauri:dev` 실패 | VS Build Tools 2022(C++ 워크로드)와 WebView2 Runtime 설치 후 터미널 재시작 |
| `cargo`를 찾을 수 없음 | [rustup](https://rustup.rs)으로 설치 후 터미널 재시작 (`%USERPROFILE%\.cargo\bin`이 PATH에 있어야 함) |
| SSH 인증 실패 | host/port/user/자격증명 확인; 서버가 해당 인증 방식을 허용하는지 확인 |
| 호스트 키 불일치 | 다른 경로로 서버 지문을 검증한 뒤 `known_hosts.json`에서 이전 항목 제거 |
| GPU가 '사용 불가'로 표시 | GPU 도구(`nvidia-smi`, `rocm-smi`, `xpu-smi`, `intel_gpu_top`) 설치 확인 — 다른 지표는 무관하게 정상 동작 |

## 로드맵 / 알려진 제한

- keyboard-interactive SSH 인증 미지원
- 디렉토리 재귀 업로드/다운로드 및 전송 이어받기 미지원
- `known_hosts.json`은 OpenSSH known_hosts 형식이 아닌 SHA-256 지문 JSON 사용
- 텔레메트리는 Linux 우선(`/proc`, `lscpu`, POSIX `df`); GPU 모니터링은 `nvidia-smi`·`rocm-smi`·`xpu-smi`·`intel_gpu_top` 사용 (AMD는 현재 `rocm-smi` 기준)

이슈와 풀 리퀘스트를 환영합니다 — 제출 전에 위의 테스트를 실행해 주세요.

## 라이선스

[MIT](./LICENSE) © GpuTerm contributors. [Tauri](https://tauri.app), [React](https://react.dev), [xterm.js](https://xtermjs.org), [ssh2](https://crates.io/crates/ssh2)로 만들어졌으며, 서드파티 라이선스는 각 저작자에게 있습니다.

# GpuTerm

[English](./README.md) | [한국어](./README.ko.md)

GpuTerm은 GPU 서버의 SSH/SFTP 세션을 관리하기 위한 Tauri + React + TypeScript + Rust 기반 데스크톱 MVP입니다. 하나의 앱에서 SSH 터미널과 SFTP 파일 관리를 제공하며, SSH로 접속한 원격 Linux 서버의 CPU, 메모리, 디스크, NVIDIA GPU 상태를 하단 상태바에서 실시간으로 확인할 수 있습니다.

현재 베타 버전: [`v1.0.3-beta`](https://github.com/fortranmentis/GPUTERM/releases/tag/v1.0.3-beta)

## 주요 기능

- host, port, username, private key path를 포함하는 로컬 SSH 세션 프로필
- 연결 시 password 사용을 지원하지만 로컬 JSON에는 저장하지 않음
- Rust `ssh2` PTY shell과 연결된 xterm.js 터미널
- 창 크기 변경에 따른 원격 PTY 크기 동기화
- SSH read chunk 사이에 나뉜 다중 바이트 문자를 보존하는 UTF-8 출력 버퍼
- SFTP 디렉터리 탐색, 업로드, 다운로드, 삭제, 새 폴더 생성
- 드래그 앤 드롭 양방향 파일 전송과 전송 큐 진행률
- 1 MiB 단위 스트리밍, 파일별 취소, 안전한 임시 다운로드 파일
- 별도 SSH 세션에서 수집하는 Linux CPU, 메모리, 디스크, NVIDIA GPU telemetry
- 1/2/5/10초 수집 주기, 표시 모드, 무시할 디스크 filesystem 설정
- SHA-256 fingerprint를 보여주는 명시적 trust-on-first-use host key 확인
- host key mismatch 차단
- 운영 및 개발 Tauri 창에 적용되는 Content Security Policy

## 프로젝트 구조

```text
src/
  components/
    RemoteTelemetryBar.tsx
    SessionSidebar.tsx
    SftpBrowser.tsx
    TerminalPane.tsx
  hooks/
    useDisconnectSession.ts
  stores/
    sessionStore.ts
  types/
    gpu.ts
    session.ts
  utils/
    format.ts
src-tauri/
  src/
    ssh/
      credentials.rs
      gpu_monitor.rs
      mod.rs
      parse_util.rs
      resource_details.rs
      session.rs
      sftp.rs
      system_monitor.rs
      terminal.rs
    lib.rs
    main.rs
```

## 설치

필수 환경:

- Node.js 20 이상
- npm 10 이상
- `cargo`, `rustc`를 포함한 Rust stable toolchain
- 운영체제별 Tauri 데스크톱 빌드 필수 항목

Windows 필수 항목:

- Microsoft Visual Studio Build Tools 2022
- Desktop development with C++ workload
- WebView2 Runtime
- Git for Windows

macOS 필수 항목:

- Xcode Command Line Tools

```bash
xcode-select --install
```

Linux 필수 패키지는 배포판에 따라 다릅니다. Debian/Ubuntu에서는 Tauri에 필요한 WebKitGTK, 빌드 도구, SSL 패키지를 설치합니다.

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

저장소를 clone합니다.

```bash
git clone https://github.com/fortranmentis/GPUTERM.git
cd GPUTERM
```

JavaScript 의존성을 설치합니다.

```bash
npm install
```

Rust toolchain이 PATH에서 인식되는지 확인합니다.

```bash
cargo --version
rustc --version
```

## 개발 실행

Tauri 데스크톱 앱 전체를 실행합니다.

```bash
npm run tauri:dev
```

Vite frontend와 네이티브 Tauri 창이 함께 실행됩니다. SSH 터미널, SFTP, 로컬 폴더 선택, telemetry 기능은 Tauri command/event가 필요하므로 이 모드에서 테스트해야 합니다.

브라우저 기반 Vite frontend만 실행하려면 다음 명령을 사용합니다.

```bash
npm run dev
```

Vite 전용 모드는 UI 레이아웃 작업에는 유용하지만 일반 브라우저에서는 폴더 선택 dialog나 Rust SSH command 같은 네이티브 Tauri API를 사용할 수 없습니다.

테스트를 실행합니다.

```bash
npm run test
cargo test --manifest-path src-tauri/Cargo.toml
```

프로덕션 검사를 실행합니다.

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

## 빌드

배포 가능한 데스크톱 패키지를 생성합니다.

```bash
npm run tauri:build
```

생성된 패키지는 다음 경로에서 확인할 수 있습니다.

```text
src-tauri/target/release/bundle
```

Windows에서는 일반적으로 `.msi`/`.exe`, macOS에서는 `.dmg`/`.app`, Linux에서는 bundle 설정에 따라 `.deb`/`.AppImage`가 생성됩니다.

## 사용 방법

앱을 실행합니다.

```bash
npm run tauri:dev
```

SSH 세션 생성:

1. 왼쪽 사이드바의 세션 입력 폼을 엽니다.
2. `host`, `port`, `username`과 password 또는 private key path를 입력합니다.
3. 세션을 저장합니다.
4. 저장된 세션을 클릭해 연결합니다.

세션 관련 참고사항:

- password는 현재 연결에만 사용되며 저장되지 않습니다.
- private key 내용은 저장하지 않고 경로만 저장합니다.
- 서버에 처음 접속하면 SHA-256 host key fingerprint를 표시하고 신뢰 여부를 묻습니다.
- 승인한 fingerprint는 `known_hosts.json`에 저장됩니다.
- 이후 서버 fingerprint가 달라지면 연결을 차단합니다.

SSH 터미널 사용:

1. 저장된 세션에 연결합니다.
2. 터미널 패널에 명령을 입력합니다.
3. 창 크기를 변경하면 원격 PTY 크기도 자동으로 갱신됩니다.

SFTP 브라우저 사용:

1. SSH 세션에 연결합니다.
2. 원격 경로 입력과 탐색 버튼으로 서버 디렉터리를 이동합니다.
3. `Local path` 옆의 `Browse...`를 눌러 OS 폴더 선택창에서 로컬 디렉터리를 선택합니다.
4. 원격 파일을 선택해 현재 로컬 디렉터리로 다운로드합니다.
5. 로컬 파일을 선택해 현재 원격 디렉터리로 업로드합니다.
6. 로컬 파일을 원격 패널로 드래그하면 업로드됩니다.
7. 원격 파일을 로컬 패널로 드래그하면 다운로드됩니다.
8. SFTP 패널에서 파일 삭제와 새 폴더 생성을 사용할 수 있습니다.

마지막으로 선택한 로컬 디렉터리는 앱을 다시 실행할 때 복원됩니다.

파일 전송 참고사항:

- 여러 파일을 한 번에 드롭할 수 있습니다.
- 파일 전체를 메모리에 올리지 않고 1 MiB chunk로 전송합니다.
- 전송 큐에 파일명, 방향, source/target path, 진행률, 상태, 파일별 오류가 표시됩니다.
- 실행 중인 전송은 파일별로 취소할 수 있습니다.
- 다운로드는 임시 파일에 먼저 기록한 뒤 성공했을 때만 최종 파일명으로 변경합니다.
- 실패하거나 취소된 다운로드는 불완전한 대상 파일을 남기지 않습니다.
- 같은 이름의 대상 파일이 있으면 전송 전에 overwrite 여부를 확인합니다.
- 디렉터리 드래그 앤 드롭은 감지하지만 현재 MVP에서는 전송하지 않습니다.

원격 telemetry 사용:

1. SSH로 Linux 서버에 연결합니다.
2. 하단바에서 CPU, RAM, Disk, GPU 수집이 시작됩니다.
3. 수집 주기를 1, 2, 5, 10초 중에서 선택합니다.
4. 표시 모드를 GPU only, System only, GPU + System 중에서 선택합니다.
5. Disk 요약을 클릭하면 전체 디스크 상세 팝오버가 열립니다.
6. CPU, RAM 또는 GPU 요약을 클릭하면 실시간 상세 정보와 상위 프로세스를 볼 수 있습니다.

NVIDIA GPU 서버에서는 각 수집 주기마다 `nvidia-smi`를 실행합니다. `nvidia-smi`가 없거나 NVIDIA GPU가 없는 경우에도 터미널 연결은 유지되며 GPU 영역만 unavailable 상태로 표시됩니다.

## 문제 해결

`npm install` 실패:

- Node.js 20 이상과 npm 10 이상인지 확인합니다.
- 의존성 설치가 손상된 경우에만 `node_modules`를 삭제한 뒤 `npm install`을 다시 실행합니다.

`cargo` 또는 `rustc`를 찾지 못함:

- rustup으로 Rust를 설치합니다.
- Cargo bin 경로가 PATH에 반영되도록 터미널을 다시 시작합니다.
- Windows 기본 경로는 일반적으로 `%USERPROFILE%\.cargo\bin`입니다.

Windows에서 `npm run tauri:dev` 실패:

- Visual Studio Build Tools 2022와 Desktop development with C++ workload를 설치합니다.
- WebView2 Runtime 설치 여부를 확인합니다.
- 빌드 도구 설치 후 터미널을 다시 시작합니다.

SSH 인증 실패:

- host, port, username, password, private key path를 확인합니다.
- 원격 서버가 password 또는 public key 인증을 허용하는지 확인합니다.
- 최초 접속 시 표시되는 SHA-256 fingerprint를 서버의 신뢰할 수 있는 경로에서 확인한 뒤 승인합니다.
- host key가 변경됐다면 서버 fingerprint를 확인한 후에만 `known_hosts.json`의 오래된 항목을 제거합니다.

SFTP 로컬 탐색 실패:

- 현재 OS 사용자가 읽을 수 있는 실제 디렉터리를 선택합니다.
- Windows에서는 `C:\Users\user\Downloads` 같은 경로를 사용할 수 있습니다.
- macOS/Linux에서는 `/Users/user/Downloads` 또는 `/home/user/Downloads` 같은 경로를 사용할 수 있습니다.

GPU metric unavailable:

- 원격 서버에 NVIDIA driver가 설치됐는지 확인합니다.
- 원격 서버에서 `nvidia-smi`를 직접 실행해 봅니다.
- NVIDIA GPU가 없는 서버에서도 CPU, memory, disk telemetry는 사용할 수 있습니다.

## 아키텍처

frontend는 `@tauri-apps/api/core`를 통해 Tauri command를 호출하고, Tauri event로 streaming update를 받습니다.

- `connect_terminal`: SSH 연결, PTY, shell을 생성하고 `terminal-output` event를 전송합니다.
- `terminal_write`: xterm 입력을 SSH channel에 전달합니다.
- `terminal_resize`: 원격 PTY 크기를 갱신합니다.
- 터미널 출력은 불완전한 UTF-8 byte sequence를 다음 read까지 보관해 분리된 다중 바이트 문자를 복원합니다.
- `system_monitor::start`: 터미널과 분리된 SSH 연결을 열어 telemetry 실패가 터미널을 막거나 끊지 않게 합니다.
- `remote-telemetry`: CPU, memory, disk, GPU와 영역별 오류를 하나의 payload로 전달합니다.
- SFTP command는 현재 활성 credential로 별도 SSH/SFTP 세션을 열고 `sftp-progress` event를 보냅니다.
- `cancel_transfer`: 다른 파일이나 터미널 세션에 영향을 주지 않고 해당 chunk 전송만 취소합니다.
- 로컬 SFTP 패널은 Tauri 기본 폴더 선택창을 사용하고 마지막 디렉터리를 앱 설정에 저장합니다.

세션 프로필은 사용자 config 디렉터리에 저장됩니다.

- Windows: `%APPDATA%/GpuTerm/sessions.json`
- macOS: `~/Library/Application Support/GpuTerm/sessions.json`
- Linux: `$XDG_CONFIG_HOME/GpuTerm/sessions.json` 또는 `~/.config/GpuTerm/sessions.json`

Host key fingerprint는 같은 디렉터리의 `known_hosts.json`에 저장됩니다. 최초 연결은 SHA-256 fingerprint에 대한 명시적 승인을 기다리며, 승인 후 저장된 값과 이후 연결의 값이 다르면 연결을 차단합니다.

최근 SFTP 로컬 디렉터리는 같은 config 디렉터리의 `app_settings.json`에 `recentLocalPath`로 저장됩니다. Password와 private key 내용은 이 설정 파일에 기록하지 않습니다.

## SFTP 로컬 브라우저

SFTP 패널의 Local path 옆에는 `Browse...` 버튼이 있습니다. Tauri dialog plugin으로 OS 기본 폴더 선택창을 열며, 선택한 경로가 존재하고 접근 가능한지 검사한 뒤 로컬 파일 목록을 다시 불러옵니다. 선택을 취소하면 기존 경로를 유지합니다.

다운로드는 선택한 로컬 디렉터리에 저장됩니다. 업로드는 로컬 파일 목록에서 선택한 파일을 현재 원격 디렉터리로 전송합니다. 경로는 운영체제 문자열을 그대로 사용하므로 Windows, macOS, Linux 경로 구분자를 지원합니다.

드래그 앤 드롭도 같은 업로드/다운로드 command를 사용합니다. 로컬 파일을 원격 패널에 놓으면 현재 원격 디렉터리로 업로드하고, 원격 파일을 로컬 패널에 놓으면 선택한 로컬 디렉터리로 다운로드합니다. 각 파일은 독립적인 전송 큐 항목과 진행률을 가집니다.

전송은 1 MiB chunk를 사용하며 파일별 취소를 지원합니다. 다운로드는 같은 디렉터리의 임시 파일에 먼저 기록하고 전체 stream이 성공했을 때만 요청한 대상 파일로 교체합니다.

## 원격 Telemetry

기본 수집 주기는 2초입니다. UI에서 1, 2, 5, 10초로 변경할 수 있고 GPU only, System only, GPU + System 표시 모드를 제공합니다.

CPU, RAM, GPU, Disk 요약은 클릭할 수 있습니다. CPU/RAM/GPU 상세 팝오버는 일반 상태바 telemetry와 별도로 수집하며, 팝오버가 열려 있는 동안에만 3초마다 갱신합니다. 상세 요청마다 별도 SSH 연결과 command channel을 사용하므로 실패해도 터미널이나 일반 telemetry loop에 영향을 주지 않습니다. `Esc` 또는 팝오버 바깥 클릭으로 닫을 수 있습니다.

CPU 수집 명령:

```bash
cat /proc/stat
cat /proc/loadavg
cat /proc/cpuinfo
nproc --all
nproc
lscpu
```

CPU 사용률은 이전과 현재 `/proc/stat` aggregate sample의 차이로 계산합니다. 첫 sample에서는 두 번째 값이 준비될 때까지 `n/a`가 표시될 수 있습니다.

메모리 수집 명령:

```bash
cat /proc/meminfo
```

메모리는 내부적으로 MiB 단위를 사용하고 UI에서는 GiB로 표시합니다. Used memory는 `MemTotal - MemAvailable`로 계산합니다.

## 리소스 상세 명령

CPU 상세 정보는 모델명, 사용률, load average, core 수, 평균 clock, uptime, logical core 사용률, CPU 사용량 상위 프로세스를 포함합니다.

```bash
cat /proc/stat
cat /proc/loadavg
cat /proc/cpuinfo
cat /proc/uptime
nproc --all
nproc
lscpu
ps -eo pid=,user=,%cpu=,%mem=,etime=,comm= --sort=-%cpu | head -n 15
```

RAM 상세 정보는 total, used, available, free, buffers, cache, swap과 RSS 사용량 상위 프로세스를 포함합니다.

```bash
cat /proc/meminfo
ps -eo pid=,user=,rss=,vsz=,%mem=,comm= --sort=-rss | head -n 15
```

GPU 상세 정보는 사용률, VRAM, 온도, 전력, fan speed, clock, PCI bus ID, persistence mode, MIG mode, 활성 compute process를 포함합니다.

```bash
nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free,fan.speed,clocks.current.graphics,clocks.current.memory,pci.bus_id,persistence_mode,mig.mode.current --format=csv,noheader,nounits
nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader,nounits
ps -p <PID_LIST> -o pid=,user=,args=
```

프로세스 소유자와 command line은 현재 SSH 사용자의 권한으로 조회합니다. Linux `/proc` 또는 process visibility 설정에 따라 다른 사용자의 command line이 보이지 않을 수 있습니다. Fan이나 MIG 같은 선택적 NVIDIA 값이 없으면 전체 패널을 실패시키지 않고 해당 항목만 unavailable로 표시합니다.

디스크 수집 명령:

```bash
df -P -T -B1
```

기본 숨김 filesystem type은 `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, `overlay`입니다. Mount point 우선순위는 `/`, `/home`, `/data`, `/mnt*`, `/media*`, 그 외 순서입니다.

하단바에는 최대 두 개의 mount point를 요약 표시합니다.

```text
Disk: / 46% · /data 43% · +2
```

Disk 영역을 클릭하면 mount point, filesystem, filesystem type, used, available, total, usage percentage를 포함하는 전체 상세 팝오버가 열립니다. 크기는 GiB 또는 TiB로 자동 변환하며, 80% 이상은 warning, 90% 이상은 critical로 표시합니다. 기본 ignore 목록에 포함된 filesystem도 팝오버의 토글로 임시 표시할 수 있습니다.

## GPU 모니터링 명령

SSH 연결 성공 후 기본 2초마다 다음 명령을 실행합니다.

```bash
nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free --format=csv,noheader,nounits
```

Rust backend가 CSV row를 frontend `GpuMetric`으로 파싱해 `RemoteTelemetry.gpu`에 포함합니다. `nvidia-smi`가 없거나, GPU가 없거나, timeout 또는 non-zero exit가 발생하면 GPU 영역에 `GPU metrics unavailable`을 표시합니다. GPU 수집 오류는 터미널과 CPU, memory, disk 수집에서 격리됩니다.

## 보안

- Password는 `sessions.json`에 저장하지 않습니다.
- Password는 활성 연결 동안 메모리에만 유지합니다.
- Private key 파일 내용은 앱 설정으로 읽거나 저장하지 않습니다.
- `CredentialStore`를 interface와 메모리 구현으로 분리해 향후 Windows Credential Manager, macOS Keychain, Linux Secret Service를 추가할 수 있습니다.
- 알 수 없는 host key는 SHA-256 fingerprint에 대한 명시적 승인을 요구합니다.
- Host key mismatch를 감지하면 연결을 차단합니다.
- Tauri 창은 필요한 local IPC와 개발 서버 endpoint만 허용하는 제한적인 Content Security Policy를 사용합니다.

## 라이선스

GpuTerm은 MIT License로 배포됩니다. 자세한 내용은 [LICENSE](./LICENSE)를 확인하세요.

Tauri, React, xterm.js, ssh2와 관련 Rust/JavaScript package를 포함한 제3자 오픈소스 의존성의 라이선스는 각 저작권자에게 유지됩니다.

## 알려진 제한사항

- command와 state는 향후 다중 탭을 고려해 session ID를 사용하지만 현재 MVP에서 완전히 연결된 활성 터미널은 하나입니다.
- Keyboard-interactive SSH 인증은 아직 지원하지 않습니다.
- SFTP 로컬 탐색은 디렉터리 단위이며 재귀적 업로드/다운로드와 디렉터리 드래그 앤 드롭은 지원하지 않습니다.
- 중단한 SFTP 전송은 resume할 수 없으며 처음부터 다시 시작해야 합니다.
- SFTP command는 안정성을 위해 매번 새 SSH 세션을 사용합니다. 향후 pooled SFTP channel을 추가할 수 있습니다.
- `known_hosts` MVP는 OpenSSH known_hosts 형식이 아니라 JSON에 SHA-256 fingerprint를 저장합니다.
- 시스템 telemetry는 Linux 우선이며 `/proc`, `nproc`, `lscpu`, GNU/POSIX 형식 `df`에 의존합니다.
- GPU monitoring은 NVIDIA GPU와 `nvidia-smi`를 전제로 합니다. NVIDIA가 없는 서버에서도 CPU, memory, disk telemetry는 표시됩니다.
- 리소스 상세 패널은 Linux 우선이며 GPU 상세에는 NVIDIA driver와 `nvidia-smi`가 추가로 필요합니다.
- 연결된 SSH 계정에 다른 프로세스를 조회할 권한이 없으면 process user/command 정보가 일부 누락될 수 있습니다.

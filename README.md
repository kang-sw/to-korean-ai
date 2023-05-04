# OpenAI API를 활용하는 한국어 번역기

프롬프트 자동 생성을 통한 임의의 언어 -> 한국어 번역 지원

## CLI 툴 사용

[`example/cli.rs`](example/cli.rs)

Release에서 바이너리 다운로드.

환경 변수에 `OPENAI_API_KEY` 추가하거나, 커맨드라인으로 `--openai-api-key <key>` 입력 후 번역할 파일
입력. 현재 plain text 파일만 번역 가능합니다.

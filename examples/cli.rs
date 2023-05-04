use std::{
    mem::replace,
    num::NonZeroUsize,
    path::Path,
    slice::SliceIndex,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, Instant},
};

use capture_it::capture;
use lib::{
    async_openai::error::OpenAIError,
    config_it::lazy_static,
    translate::{self, TranslationInputContext},
};
use parking_lot::Mutex;
use tokio::{
    sync::{mpsc, oneshot},
    task::spawn_blocking,
};

#[derive(clap::Parser)]
struct Args {
    /// File to open
    file_name: String,

    /// API key from commandline
    #[arg(long)]
    openai_api_key: Option<String>,

    /// Line number to start parsing
    #[arg(short = 'O', long, default_value_t = 0)]
    offset: usize,

    /// Line count to finish parsing    
    #[arg(short = 'c', long)]
    count: Option<NonZeroUsize>,

    /// Output file path
    #[arg(short, long = "out")]
    output: Option<String>,

    /// Line batch count
    #[arg(short, long, default_value_t = NonZeroUsize::new(100).unwrap())]
    line_batch: NonZeroUsize,

    /// Maximum characters for single translation. This is the most important parameter.
    #[arg(short = 'M', long, default_value_t = 1000)]
    max_chars: usize,

    /// Number of parallel translation jobs. This is affected by OpenAI API rate limit.
    #[arg(short, long, default_value_t = 5)]
    jobs: usize,

    /// Disables leading context features
    #[arg(long)]
    disable_leading_context: bool,

    /// Number of maximum leading context lines.
    #[arg(short = 'L', long, default_value_t = 5)]
    max_leading_context: usize,

    /// Number of maximum empty lines to separate context.
    ///
    /// For example, if this value is 2, then after 3 lines of empty lines, next line will be
    /// treated as different context.
    #[arg(short = 'C', long = "chapter", default_value_t = 3)]
    chapter_sep: usize,

    /// Number of line separator between every lines.
    #[arg(long, default_value_t = 1)]
    line_sep: usize,

    /// Print source text line at the same time.
    #[arg(short = 'S', long = "print-source")]
    print_source_block: bool,

    /// Whether to overwrite existing output file.
    #[arg(long)]
    overwrite: bool,

    /// Model temperature
    #[arg(long)]
    temperature: Option<f32>,

    /// Retry count on failure
    #[arg(long, default_value_t = 3)]
    retry: usize,

    /// Retry interval in milliseconds
    #[arg(long, default_value_t = 30_000)]
    retry_after: usize,
}

impl Args {
    fn get() -> &'static Args {
        lazy_static! {
            static ref BODY: Args = <Args as clap::Parser>::parse();
        };
        &BODY
    }
}

fn main() {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn,cli=debug");
    }

    env_logger::init();

    let openai_key = std::env::var("OPENAI_API_KEY")
        .ok()
        .or_else(|| Args::get().openai_api_key.clone())
        .expect("API key not supplied: OPENAI_API_KEY env or --openai-api-key=?");
    let h = translate::Instance::new(&openai_key);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    tokio::task::LocalSet::new().block_on(&rt, async_main(h.into()));
}

/* ---------------------------------------------------------------------------------------------- */
/*                                           ASYNC MAIN                                           */
/* ---------------------------------------------------------------------------------------------- */
async fn async_main(transl: Arc<translate::Instance>) {
    let args = Args::get();
    let setting = translate::Settings::builder()
        .source_lang(lib::lang::Language::Japanese)
        .profile(Arc::new(lib::lang::profiles::KoreanV1))
        .temperature(args.temperature.or(Some(0.1)))
        .build();

    let setting = Arc::new(setting);

    let (tx_task, rx_task) = tokio::sync::mpsc::unbounded_channel();
    let task_tickets = Arc::new(tokio::sync::Semaphore::new(args.jobs));

    let lines = if let Some(count) = args.count {
        file_read_lines(args.offset..args.offset + count.get())
    } else {
        file_read_lines(args.offset..)
    };

    let Some(mut source_lines) = lines else {
        log::error!("file_read_lines(..) failed");
        return
    };

    log::info!("{} lines will be processed", source_lines.len());

    // 출력 테스크 처리기 ... File I/O 등 처리
    let output_task = tokio::task::spawn_local(output_task(rx_task));

    // 처리할 라인
    let mut proc_lines = Vec::new();
    let mut leading_context = Vec::new();
    let mut empty_line_count = 0;
    let mut new_context = false;
    let initial_num_lines = source_lines.len();

    // ctrl-c 처리.
    let stop_queued = Arc::new(AtomicBool::new(false));
    tokio::spawn(capture!([stop_queued], async move {
        let _ = tokio::signal::ctrl_c().await;
        log::warn!("Ctrl-C received. The program will be stopped after current batch request.");
        log::info!("To stop quickly, press Ctrl-C again.");
        stop_queued.store(true, Relaxed);
    }));

    // 주 루프 -> 모든 라인 처리 시점까지
    while source_lines.is_empty() == false {
        let permit = task_tickets.clone().acquire_owned().await.unwrap();

        if stop_queued.load(Relaxed) {
            log::info!("Stopping ...");
            break;
        }

        proc_lines.clear();
        let mut char_count = 0;
        let mut batch_count = 0;

        while batch_count < args.line_batch.get() {
            if let Some(line) = source_lines.first() {
                // 적어도 한 개의 문장이 번역 예정 + 이 문장 포함 시 최대 문자 수 초과 -> escape
                if proc_lines.is_empty() == false && char_count + line.len() > args.max_chars {
                    break;
                }

                // 공백이 아닌 라인만 집어넣는다.
                if let Some(line) = Some(line.trim()).filter(|x| x.is_empty() == false) {
                    empty_line_count = 0;

                    if replace(&mut new_context, false) {
                        log::debug!(
                            "  SEND) * new context .. remaining lines: {}",
                            source_lines.len()
                        );
                        tx_task.send(OutputTask::ContextSeparator).ok();

                        // 새 컨텍스트 ... 이전 컨텍스트 정보는 필요없게 되었음.
                        leading_context.clear();
                    }

                    proc_lines.push(line);
                    char_count += line.len();
                    batch_count += 1;
                } else {
                    empty_line_count += 1;

                    // 공백인 라인이 일정 횟수 이상 반복
                    if empty_line_count == args.chapter_sep {
                        // 이전 컨텍스트가 있을 때에만 새 컨텍스트로 간주.
                        if leading_context.is_empty() == false || proc_lines.is_empty() == false {
                            new_context = true;
                        }

                        if proc_lines.is_empty() == false {
                            log::debug!(
                                "  SEND) * context switched ... flushing at {} lines ",
                                proc_lines.len()
                            );
                            break;
                        }
                    }
                }

                // Proceed once
                source_lines = source_lines.split_at(1).1;
            } else {
                break;
            }
        }

        log::debug!(
            "SEND) LINE={proc}++{pending} in total={total}, offset={ofst}{lead}, task={task}",
            proc = initial_num_lines - source_lines.len(),
            pending = proc_lines.len(),
            total = initial_num_lines,
            ofst = args.offset + initial_num_lines - source_lines.len(),
            lead = leading_context.is_empty().then_some("").unwrap_or("(*)"),
            task = args.jobs - task_tickets.available_permits(),
        );
        let (tx, rx) = oneshot::channel();

        if proc_lines.iter().copied().all(|x| has_lang_ch(x) == false) {
            log::debug!("  SEND) * no target lang character detected. skip translation");
            tx_task.send(OutputTask::WriteAsIs(proc_lines.clone())).ok();

            continue;
        }

        // 바운드 큐에 작업을 추가한다. 만약 이전 작업이 지연된 경우 await은 반환하지 않으므로
        // -> 자연스러운 부하 제어 가능
        tx_task.send(OutputTask::PendingTranslation(rx)).ok();

        // 비동기적으로 번역 태스크 실행
        let task = translate_task(
            transl.clone(),
            permit,
            setting.clone(),
            (!args.disable_leading_context)
                .then(|| leading_context.clone())
                .unwrap_or_default(),
            proc_lines.clone(),
            tx,
        );

        tokio::task::spawn_local(task);

        // 현재 컨텍스트를 이전 컨텍스트로 교체한다.
        let num_prev = args.max_leading_context.min(proc_lines.len());
        leading_context.splice(
            ..,
            proc_lines[proc_lines.len() - num_prev..].iter().copied(),
        );
    }

    // Wait for output task to be finished.
    log::info!("Waiting for remaining translation jobs to be finished ...");
    drop(tx_task);
    let _ = output_task.await;

    // Let all output to be flushed.
    spawn_blocking(|| {
        let _ = output().lock().flush();
    })
    .await
    .ok();

    // Finish notification
    log::info!(
        "finished. Current line offset: {} (started from {}, {} lines processed)",
        args.offset + initial_num_lines - source_lines.len(),
        args.offset,
        initial_num_lines - source_lines.len()
    );
}

/* ----------------------------------------- Output Task ---------------------------------------- */

#[derive(Debug, Clone, Copy, derive_more::IsVariant)]
enum Style {
    Plain,
    Markdown,
}

enum OutputTask {
    PendingTranslation(oneshot::Receiver<TranslationTask>),
    WriteAsIs(Vec<&'static str>),
    ContextSeparator,
}

async fn output_task(mut rx_result: mpsc::UnboundedReceiver<OutputTask>) {
    let mut total_prompt_count = 0;
    let mut total_reply_count = 0;
    let mut req_counter = 0;
    let start_at = Instant::now();

    let output_style = match Args::get()
        .output
        .as_ref()
        .map(|x| x.as_str())
        .and_then(|x| Path::new(x).extension().and_then(|x| x.to_str()))
        .unwrap_or("txt")
    {
        "md" => Style::Markdown,
        _ => Style::Plain,
    };

    log::info!("output style set to: {output_style:?}");

    while let Some(x) = rx_result.recv().await {
        match x {
            OutputTask::PendingTranslation(rx) => {
                let Ok(result) = rx.await else {
                    log::warn!("Translation task failed");
                    continue;
                };

                req_counter += 1;
                total_prompt_count += result.content.num_prompt_tokens;
                total_reply_count += result.content.num_compl_tokens;
                let delta_time = start_at.elapsed().as_secs_f64();
                let tok_rate = (total_prompt_count + total_reply_count) as f64 / delta_time;
                let req_rate = req_counter as f64 / delta_time;

                log::debug!(
                    concat!(
                        "WRITE) [{delta:.1}s] +{line} line",
                        " in {time:.2}s,",
                        " prm={total_prompt}(+{added_prompt}),",
                        " rep={total_reply}(+{added_reply}),",
                        " tpm={tok_r:.1}, rpm={req_r:.2}"
                    ),
                    delta = delta_time,
                    line = result.content.lines().len(),
                    time = result.start_at.elapsed().as_secs_f64(),
                    total_prompt = total_prompt_count,
                    added_prompt = result.content.num_prompt_tokens,
                    total_reply = total_reply_count,
                    added_reply = result.content.num_compl_tokens,
                    tok_r = tok_rate * 60.0,
                    req_r = req_rate * 60.0,
                );

                spawn_blocking(move || {
                    let mut out = output().lock();

                    if Args::get().print_source_block {
                        for src in result.src {
                            match output_style {
                                Style::Markdown => {
                                    let _ = out.write_fmt(format_args!("> {}\n", src.trim()));

                                    for _ in 0..Args::get().line_sep.max(1) {
                                        let _ = out.write_all(b"> \n");
                                    }
                                }
                                Style::Plain => {
                                    let _ = out.write_all(src.trim().as_bytes());

                                    for _ in 0..Args::get().line_sep + 1 {
                                        let _ = out.write_all(b"\n");
                                    }
                                }
                            }
                        }

                        match output_style {
                            Style::Markdown => {
                                // Insert empty line between commentary block and the content
                                let _ = out.write_all(b"\n");
                            }
                            Style::Plain => {}
                        }
                    }

                    for (_, line) in result.content.lines() {
                        assert!(!line.is_empty());
                        let _ = out.write_all(line.trim().as_bytes());

                        let mut num_newline = Args::get().line_sep + 1;

                        if output_style.is_markdown() {
                            num_newline = num_newline.max(2);
                        }

                        for _ in 0..num_newline {
                            let _ = out.write_all(b"\n");
                        }
                    }

                    let _ = out.flush();
                })
                .await
                .ok();
            }

            OutputTask::WriteAsIs(x) => {
                spawn_blocking(move || {
                    let mut out = output().lock();

                    for line in x {
                        let _ = out.write_all(line.as_bytes());
                        let _ = out.write_all(b"\n");
                    }

                    let _ = out.flush();
                })
                .await
                .ok();
            }

            OutputTask::ContextSeparator => {
                spawn_blocking(move || {
                    let mut out = output().lock();

                    match output_style {
                        Style::Plain => (0..Args::get().chapter_sep + 1).for_each(|_| {
                            out.write_all(b"\n").ok();
                        }),

                        Style::Markdown => {
                            out.write_all(b"\n\n---\n\n").ok();
                        }
                    }

                    let _ = out.flush();
                })
                .await
                .ok();
            }
        }
    }

    log::info!(
        "Output task finished. Prompt tokens: {}, Reply tokens: {} => {} tokens will be charged",
        total_prompt_count,
        total_reply_count,
        total_reply_count + total_prompt_count
    );
}

/* ----------------------------------------- Sender Task ---------------------------------------- */

struct TranslationTask {
    src: Vec<&'static str>,
    start_at: Instant,
    content: translate::TranslationResult,
}

async fn translate_task(
    h: Arc<translate::Instance>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    setting: Arc<translate::Settings>,
    leading_context: Vec<&'static str>,
    sources: Vec<&'static str>,
    reply: oneshot::Sender<TranslationTask>,
) {
    let leading_ctx = Some(leading_context)
        .filter(|x| !x.is_empty())
        .map(|x| x.join("\n"));

    let input_ctx = TranslationInputContext::builder()
        .leading_content(leading_ctx.as_ref().map(|x| x.as_str()))
        .build();

    let start_at = Instant::now();
    let args = Args::get();
    let mut tries = 0;

    loop {
        match h
            .translate(&input_ctx, &mut sources.iter().copied(), &setting)
            .await
        {
            Ok(result) => {
                let _ = reply.send(TranslationTask {
                    start_at,
                    src: sources,
                    content: result,
                });

                break;
            }

            Err(translate::Error::OpenAI(e @ OpenAIError::ApiError(..))) => {
                // TODO: Find continue condition ... -> For example, temporary rate limit
                log::error!("OpenAI: {e:#}");
            }

            Err(e) => {
                log::error!("failed to translate batch: {e:#}");
                log::debug!("source line was: {sources:#?}");
            }
        };

        if tries + 1 < args.retry {
            tries += 1;
            log::info!("retrying in {}ms ...", args.retry_after);
            tokio::time::sleep(Duration::from_millis(args.retry_after as _)).await;
        } else {
            log::warn!("all retries exhausted. skipping this batch");
            break;
        }
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                         UTILITY METHODS                                        */
/* ---------------------------------------------------------------------------------------------- */
fn file_read_lines(
    line_range: impl SliceIndex<[String], Output = [String]>,
) -> Option<&'static [String]> {
    fn __lines() -> Option<Vec<String>> {
        let args = Args::get();
        let sample_path = &args.file_name;
        log::debug!("sample_path: {sample_path:?}");

        if !std::path::Path::new(&sample_path).exists() {
            log::warn!("ignoring test: sample file not found");
            None
        } else {
            let content = std::fs::read_to_string(sample_path).ok()?;
            let content = content
                .lines()
                .map(|x| x.replace("\u{3000}", " "))
                .collect::<Vec<_>>();

            Some(content)
        }
    }

    lazy_static! {
        static ref LINES: Option<Vec<String>> = __lines();
    }

    LINES.as_ref()?.get(line_range)
}

type BoxedWrite = Box<dyn std::io::Write + Send + Sync>;

fn output() -> &'static Mutex<BoxedWrite> {
    lazy_static! {
        static ref OUTPUT: Mutex<BoxedWrite> = {
            let args = Args::get();
            log::info!(
                "Creating file with {} mode ...",
                args.overwrite.then_some("overwrite").unwrap_or("append")
            );

            let x: BoxedWrite = if let Some(path) = args.output.as_ref() {
                Box::new(
                    std::fs::OpenOptions::new()
                        .append(!args.overwrite)
                        .truncate(args.overwrite)
                        .read(false)
                        .write(true)
                        .create(true)
                        .open(path)
                        .expect("Opening file failed"),
                )
            } else {
                Box::new(std::io::stdout())
            };

            Mutex::new(x)
        };
    }

    &*OUTPUT
}

fn has_lang_ch(x: &str) -> bool {
    x.chars().any(|c| {
        // ** CHECK CHARACTER IS VALID CJK + ASCII **

        // Check character is valid Hangul
        let is_hangul = c >= '\u{AC00}' && c <= '\u{D7AF}';

        // Check character is valid Kana
        let is_kana = c >= '\u{3040}' && c <= '\u{309F}';

        // Check character is valid Kanji
        let is_kanji = c >= '\u{3000}' && c <= '\u{303F}';

        // Check character is valid Hiragana
        let is_hiragana = c >= '\u{3040}' && c <= '\u{309F}';

        // Check character is valid Katakana
        let is_katakana = c >= '\u{30A0}' && c <= '\u{30FF}';

        // Check character is valid ASCII
        let is_ascii = c >= '\u{0020}' && c <= '\u{007E}';

        // Check character is valid CJK
        let is_cjk = is_hangul || is_kana || is_kanji || is_hiragana || is_katakana;

        // Check character is valid
        is_cjk || is_ascii
    })
}

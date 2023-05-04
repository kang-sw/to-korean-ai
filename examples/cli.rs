use std::{
    num::NonZeroUsize,
    path::Path,
    slice::SliceIndex,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc,
    },
    time::Instant,
};

use capture_it::capture;
use lib::{
    async_openai::error::OpenAIError,
    config_it::lazy_static,
    translate::{self, TranslationInputContext},
};
use log::LevelFilter;
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
    #[arg(long, default_value_t = 0)]
    offset: usize,

    /// Line count to finish parsing    
    #[arg(long)]
    count: Option<NonZeroUsize>,

    /// Output file path
    #[arg(short, long = "out")]
    output: Option<String>,

    /// Line batch count
    #[arg(short, long = "batch", default_value_t = NonZeroUsize::new(100).unwrap())]
    batch_size: NonZeroUsize,

    /// Maximum characters for single translation. This is the most important parameter.
    #[arg(short = 'M', long, default_value_t = 1512)]
    max_chars: usize,

    /// Number of parallel translation jobs. This is affected by OpenAI API rate limit.
    #[arg(short, long, default_value_t = 10)]
    jobs: usize,

    /// Disables leading context features
    #[arg(long)]
    disable_leading_context: bool,

    /// Number of maximum leading context lines.
    #[arg(long, default_value_t = 5)]
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
    #[arg(long = "print-source")]
    print_source_block: bool,

    #[arg(long)]
    overwrite: bool,
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
    let setting = translate::Settings::builder()
        .source_lang(lib::lang::Language::Japanese)
        .profile(Arc::new(lib::lang::profiles::KoreanV1))
        .build();

    let setting = Arc::new(setting);
    let args = Args::get();

    let (tx_task, rx_task) = tokio::sync::mpsc::channel(args.jobs);
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
        if stop_queued.load(Relaxed) {
            log::info!("Stopping ...");
            break;
        }

        proc_lines.clear();
        let mut char_count = 0;
        let mut batch_count = 0;

        while batch_count < args.batch_size.get() {
            if let Some(line) = source_lines.first() {
                // 적어도 한 개의 문장이 번역 예정 + 이 문장 포함 시 최대 문자 수 초과 -> escape
                if proc_lines.is_empty() == false && char_count + line.len() > args.max_chars {
                    break;
                }

                // 공백이 아닌 라인만 집어넣는다.
                if let Some(line) = Some(line.trim()).filter(|x| x.is_empty() == false) {
                    empty_line_count = 0;

                    proc_lines.push(line);
                    char_count += line.len();
                    batch_count += 1;
                } else {
                    // 공백인 라인이 일정 횟수 이상 반복되면 새 문맥으로 교체한다.
                    empty_line_count += 1;

                    if empty_line_count == args.chapter_sep {
                        log::debug!("  * new context .. remaining lines: {}", source_lines.len());
                        if leading_context.is_empty() == false {
                            log::debug!("  * has active context ... inserting new separator.");
                            let _ = tx_task.send(OutputTask::ContextSeparator).await;
                            leading_context.clear();
                        }

                        if proc_lines.is_empty() == false {
                            log::debug!("  * breaking out of context earlier");
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
            "lines: {}(pending {})/{}, offset {}{}",
            initial_num_lines - source_lines.len(),
            proc_lines.len(),
            initial_num_lines,
            args.offset + initial_num_lines - source_lines.len(),
            leading_context
                .is_empty()
                .then_some("")
                .unwrap_or(", has leading context"),
        );
        let (tx, rx) = oneshot::channel();

        // 바운드 큐에 작업을 추가한다. 만약 이전 작업이 지연된 경우 await은 반환하지 않으므로
        // -> 자연스러운 부하 제어 가능
        let _ = tx_task.send(OutputTask::PendingTranslation(rx)).await;

        // 비동기적으로 번역 태스크 실행
        let task = translate_task(
            transl.clone(),
            setting.clone(),
            (!args.disable_leading_context)
                .then(|| leading_context.clone())
                .unwrap_or_default(),
            proc_lines.clone(),
            tx,
        );

        tokio::task::spawn_local(task);

        // 현재 컨텍스트를 이전 컨텍스트로 교체한다.
        let proc_lines = proc_lines.iter().rev().skip(args.max_leading_context).rev();
        leading_context.splice(.., proc_lines.copied());
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

#[derive(Debug, Clone, Copy)]
enum Style {
    Plain,
    Markdown,
}

enum OutputTask {
    PendingTranslation(oneshot::Receiver<TranslationTask>),
    ContextSeparator,
}

async fn output_task(mut rx_result: mpsc::Receiver<OutputTask>) {
    let mut total_prompt_count = 0;
    let mut total_reply_count = 0;

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

                total_prompt_count += result.content.num_prompt_tokens;
                total_reply_count += result.content.num_compl_tokens;

                log::debug!(
                    concat!(
                        "writing {line} lines.",
                        " {time:.2} seconds elapsed.",
                        " {total_prompt} (+{added_prompt}) prompt,",
                        " {total_reply} (+{added_reply}) reply tokens"
                    ),
                    line = result.content.num_compl_tokens,
                    time = result.start_at.elapsed().as_secs_f64(),
                    total_prompt = total_prompt_count,
                    added_prompt = result.content.num_prompt_tokens,
                    total_reply = total_reply_count,
                    added_reply = result.content.num_compl_tokens,
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

                        if matches!(output_style, Style::Markdown) {
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

            OutputTask::ContextSeparator => {
                spawn_blocking(move || {
                    let mut out = output().lock();
                    for _ in 0..Args::get().chapter_sep + 1 {
                        let _ = out.write_all(b"\n");
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

    loop {
        break match h
            .translate(&input_ctx, &mut sources.iter().copied(), &setting)
            .await
        {
            Ok(result) => {
                let _ = reply.send(TranslationTask {
                    start_at,
                    src: sources,
                    content: result,
                });
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
            let content = content.lines().map(|x| x.to_owned()).collect::<Vec<_>>();

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

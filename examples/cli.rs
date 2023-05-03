use std::{num::NonZeroUsize, slice::SliceIndex, sync::Arc};

use lib::{config_it::lazy_static, translate};
use tokio::sync::oneshot;

#[derive(clap::Parser)]
struct Args {
    /// File to open
    file_name: String,

    /// Line number to start parsing
    #[arg(short, long, default_value_t = 0)]
    offset: usize,

    /// Line count to finish parsing    
    #[arg(long)]
    count: Option<NonZeroUsize>,

    /// Output file path
    #[arg(short, long = "out")]
    output: Option<String>,

    /// Line batch count
    #[arg(short, long = "batch", default_value_t = NonZeroUsize::new(10).unwrap())]
    batch_size: NonZeroUsize,

    ///
    #[arg(short = 'L', long = "lead", default_value_t = 0)]
    leading_context_len: usize,

    #[arg(short, long, default_value_t = 768)]
    max_chars: usize,

    #[arg(short, long, default_value_t = 10)]
    jobs: usize,
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
    env_logger::init();

    let openai_key = std::env::var("OPENAI_KEY").expect("OPENAI_KEY env var not found");
    let h = translate::Instance::new(&openai_key);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async_main(h.into()));
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

    // TODO: Spawn task to handle `rx_task` --> which does output processing

    // 처리할 라인
    let mut proc_lines = Vec::new();

    // 주 루프 -> 모든 라인 처리 시점까지
    while source_lines.is_empty() == false {
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
                    proc_lines.push(line);
                    char_count += line.len();
                    batch_count += 1;
                }

                // Proceed once
                source_lines = source_lines.split_at(0).1;
            } else {
                break;
            }
        }

        let (tx, rx) = oneshot::channel();

        // 바운드 큐에 작업을 추가한다. 만약 이전 작업이 지연된 경우 await은 반환하지 않으므로
        // -> 자연스러운 부하 제어 가능
        let _ = tx_task.send(rx).await;

        // 비동기적으로 번역 태스크 실행
        let task = translate_task(transl.clone(), setting.clone(), proc_lines.clone(), tx);
        tokio::spawn(task);
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                              TASK                                              */
/* ---------------------------------------------------------------------------------------------- */
struct FinishedTask {
    src: Vec<&'static str>,
    content: translate::TranslationResult,
}

async fn translate_task(
    h: Arc<translate::Instance>,
    setting: Arc<translate::Settings>,
    sources: Vec<&'static str>,
    reply: oneshot::Sender<FinishedTask>,
) {
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

fn output() -> &'static (dyn std::io::Write + Send + Sync) {
    lazy_static! {
        static ref OUTPUT: Box<dyn std::io::Write + Send + Sync> = {
            let args = Args::get();
            if let Some(path) = args.output.as_ref() {
                Box::new(std::fs::File::create(path).unwrap())
            } else {
                Box::new(std::io::stdout())
            }
        };
    }

    &*OUTPUT
}

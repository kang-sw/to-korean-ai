//! 번역 작업을 위한 OpenAI API 프롬프트 추상화

use std::{borrow::Cow, sync::Arc};

use async_openai::{
    error::OpenAIError,
    types::{ChatCompletionRequestMessage, CreateChatCompletionRequest, Role},
};
use compact_str::CompactString;
use default::default;

use crate::lang::{Language, PromptProfile};

static_assertions::assert_impl_all!(Instance: Send, Sync, Unpin);

/// OpenAI API 호출 관리
#[derive(Debug, Clone)]
pub struct Instance {
    ai: async_openai::Client,
}

#[allow(non_camel_case_types)]
#[derive(Debug, derive_more::Display, Default, Clone, Copy)]
pub enum ChatModel {
    #[display(fmt = "gpt-3.5-turbo")]
    #[default]
    Gpt_3_5_Turbo,

    #[display(fmt = "gpt-4")]
    Gpt_4,
}

/// 번역에 필요한 정적 세팅(모델 정보, 토큰 길이 등)을 포함합니다.
#[derive(Debug, typed_builder::TypedBuilder, Clone)]
pub struct Settings {
    source_lang: Language,
    profile: Arc<dyn PromptProfile>,

    #[builder(default)]
    model: ChatModel,

    #[builder(default, setter(strip_option))]
    temperature: Option<f32>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("OpenAI API error: {0}")]
    OpenAI(#[from] OpenAIError),

    #[error("No choice found")]
    NoChoice,

    #[error("Failed to parse output noun list during ProperNoun generation")]
    NounListParseFailed,

    #[error("Token length limit exceeded.")]
    TokenLengthLimit,

    #[error("Line count mismatch: expected {expected}, actual {actual}")]
    LineCountMismatch { expected: usize, actual: usize },
}

/* ------------------------------------------ Core Ops ------------------------------------------ */
struct CallResult {
    assistant_reply: String,
    num_prompt_tokens: usize,
    num_compl_tokens: usize,
}

impl Instance {
    pub fn new(api_key: &str) -> Self {
        Self {
            ai: async_openai::Client::new().with_api_key(api_key),
        }
    }

    async fn _call(&self, req: CreateChatCompletionRequest) -> Result<CallResult, Error> {
        let rep = self.ai.chat().create(req).await?;
        let msg = rep.choices.into_iter().next().ok_or(Error::NoChoice)?;

        if let Some(true) = msg.finish_reason.as_ref().map(|x| x == "length") {
            return Err(Error::TokenLengthLimit);
        }

        let (n_prompt, n_compl) = rep
            .usage
            .map(|x| (x.prompt_tokens, x.completion_tokens))
            .ok_or(Error::NoChoice)?;

        Ok(CallResult {
            assistant_reply: msg.message.content,
            num_prompt_tokens: n_prompt as _,
            num_compl_tokens: n_compl as _,
        })
    }
}

fn _gen<'a>(role: Role, msg: impl Into<Cow<'a, str>>) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage {
        role,
        content: msg.into().into(),
        ..default()
    }
}

/* ---------------------------------------- Proper Nouns ---------------------------------------- */

#[derive(custom_debug_derive::Debug)]
pub struct ProperNounRetrievalResult {
    pub nouns: Vec<CompactString>,
    pub num_prompt_tokens: usize,
    pub num_compl_tokens: usize,

    #[debug(skip)]
    _no_build: (),
}

impl Instance {
    pub async fn retrieve_proper_nouns(
        &self,
        content: &str,
        opt: &Settings,
    ) -> Result<ProperNounRetrievalResult, Error> {
        let mut req = opt.new_chat_req();
        let lang = opt.source_lang;

        req.messages.push(_gen(
            Role::System,
            opt.profile.proper_noun_instruction(lang),
        ));

        req.messages.push(_gen(Role::User, content));
        let rep = self._call(req).await?;

        let nouns = opt
            .profile
            .parse_proper_noun_output(lang, &rep.assistant_reply)
            .ok_or(Error::NounListParseFailed)?;

        Ok(ProperNounRetrievalResult {
            nouns,
            num_prompt_tokens: rep.num_prompt_tokens,
            num_compl_tokens: rep.num_compl_tokens,
            _no_build: (),
        })
    }
}

impl Settings {
    fn new_chat_req(&self) -> CreateChatCompletionRequest {
        let mut req = CreateChatCompletionRequest::default();
        req.model = self.model.to_string();
        req.temperature = self.temperature;
        req.messages.reserve(10);
        req
    }
}

/* --------------------------------------- Translation Ops -------------------------------------- */

/// 번역기에 제공할 입력 컨텍스트
///
/// TODO: 등장 인물 관계 정보?
#[derive(typed_builder::TypedBuilder, Debug)]
pub struct TranslationInputContext<'a> {
    /// 단순히 앞부분의 원문입니다. 적당한 양을 잘라서 넣으면, 앞부분의 맥락을 파악하기 위한
    /// 프롬프트를 추가적으로 생성합니다.
    #[builder(default, setter(strip_option))]
    leading_content: Option<&'a str>,

    /// 고유 명사의 Dictionary 정보입니다.
    #[builder(default = &[])]
    dictionary: &'a [(&'a str, &'a str)],
}

#[derive(Debug, Clone, Copy)]
pub struct LineDesc {
    src_index: usize,
    byte_offset: usize,
    byte_size: usize,
}

/// 번역 결과를 담습니다.
#[derive(custom_debug_derive::Debug, Clone)]
pub struct TranslationResult {
    /// 번역된 문장의 인덱스 및 내용을 담습니다.
    source_string: String,

    /// (Position, Size)
    lines: Vec<LineDesc>,

    /// 번역에 사용한 토큰 개수입니다.
    pub num_prompt_tokens: usize,
    pub num_compl_tokens: usize,

    #[debug(skip)]
    _no_build: (),
}

impl TranslationResult {
    pub fn lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.lines.iter().map(move |x| {
            let s = &self.source_string[x.byte_offset..][..x.byte_size];
            (x.src_index, s)
        })
    }
}

impl Instance {
    /// 제공된 문장에 대해 한 번의 번역 작업을 수행합니다.
    ///
    /// - `content`: 번역할 문장의 리스트입니다.
    ///
    pub async fn translate(
        &self,
        input_ctx: &TranslationInputContext<'_>,
        content: &mut dyn Iterator<Item = &str>,
        opt: &Settings,
    ) -> Result<TranslationResult, Error> {
        let mut req = opt.new_chat_req();
        let lang = opt.source_lang;

        // 번역기 지시사항 전달
        req.messages
            .push(_gen(Role::System, opt.profile.trans_instruction(lang)));

        // 부록에 고유 명사 사전 정의
        {
            let dict_prompt = opt
                .profile
                .trans_appendix_proper_noun_dict(lang, input_ctx.dictionary);

            if dict_prompt.is_empty() == false {
                req.messages.push(_gen(Role::System, dict_prompt));
            }
        }

        // 부록에 맥락 파악을 위한 원문 추가
        if let Some(x) = input_ctx
            .leading_content
            .map(|x| opt.profile.trans_appendix_leading_context_content(lang, x))
            .filter(|x| x.is_empty() == false)
        {
            req.messages.push(_gen(Role::System, x));
        }

        // 번역할 문장 작성
        let (src_lines, content) = content
            .enumerate()
            .filter_map(|(i, x)| Some((i, x.trim())).filter(|(_, x)| x.is_empty() == false))
            .map(|(i, x)| (i, x.trim()))
            .fold(
                (Vec::with_capacity(32), String::with_capacity(1024)),
                |(mut lines, mut base), (src_line_num, x)| {
                    lines.push(src_line_num);

                    base.push_str(x);
                    base.push_str("\n\n");
                    (lines, base)
                },
            );

        req.messages.push(_gen(Role::User, content));

        // 번역 요청
        let rep = self._call(req).await?;
        let base = rep.assistant_reply.as_bytes().as_ptr();
        let mut src_liner = src_lines.iter().copied();

        let result = TranslationResult {
            lines: rep
                .assistant_reply
                .lines()
                .filter_map(|x| Some(x.trim()).filter(|x| x.is_empty() == false))
                .map(|x| x.as_bytes())
                .map(|x| LineDesc {
                    src_index: src_liner.next().unwrap_or(usize::MAX),
                    byte_offset: x.as_ptr() as usize - base as usize,
                    byte_size: x.len(),
                })
                .collect(),
            source_string: rep.assistant_reply,
            num_prompt_tokens: rep.num_prompt_tokens,
            num_compl_tokens: rep.num_compl_tokens,
            _no_build: default(),
        };

        if result.lines.len() != src_lines.len() {
            return Err(Error::LineCountMismatch {
                expected: src_lines.len(),
                actual: result.lines.len(),
            });
        }

        Ok(result)
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                          TEST SECTION                                          */
/* ---------------------------------------------------------------------------------------------- */
#[cfg(test)]
mod __test {
    use std::{future::Future, sync::Arc};

    use crate::lang::{self, Language};

    use super::{Settings, TranslationInputContext};

    fn exec_test<S, F>(scope: S)
    where
        F: Future<Output = ()>,
        S: FnOnce(super::Instance) -> F,
    {
        let Ok(api_key)= std::env::var("OPENAI_API_KEY_TEST") else {
            log::info!("ignoring test: OPENAI_API_KEY_TEST is not set");
            return;
        };

        let inst = super::Instance::new(&api_key);

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(scope(inst));
    }

    /// 저작권 문제로 텍스트 내용을 레포에 포함시키지 않음. '.cargo' 디렉터리 아래에 'test_content'
    /// 파일이 없다면 테스트를 모두 제낀다.
    fn try_find_jp_sample(line_range: std::ops::Range<usize>) -> Option<String> {
        let manif_dir = env!("CARGO_MANIFEST_DIR");
        let sample_path = format!("{}/../.cargo/jp-sample.txt", manif_dir);
        log::debug!("sample_path: {sample_path:?}");

        if !std::path::Path::new(&sample_path).exists() {
            log::warn!("ignoring test: sample file not found");
            None
        } else {
            let content = std::fs::read_to_string(sample_path).ok()?;
            let content = content
                .lines()
                .skip(line_range.start)
                .take(line_range.end - line_range.start)
                .collect::<Vec<_>>()
                .join("\n");

            Some(content)
        }
    }

    #[test_log::test]
    #[ignore]
    fn run_noun_retr() {
        let Some(content) = try_find_jp_sample(0..10) else { return };

        exec_test(|h| async move {
            let setting = Settings::builder()
                .source_lang(Language::Japanese)
                .profile(Arc::new(lang::profiles::KoreanV1))
                .build();

            let res = h.retrieve_proper_nouns(&content, &setting).await;
            let _ = dbg!(res);
        });
    }

    #[test_log::test]
    #[ignore]
    fn run_translate_basic() {
        let Some(content) = try_find_jp_sample(0..10) else { return };

        exec_test(|h| async move {
            let setting = Settings::builder()
                .source_lang(Language::Japanese)
                .profile(Arc::new(lang::profiles::KoreanV1))
                .build();

            let input = TranslationInputContext::builder().build();
            let res = h
                .translate(&input, &mut content.lines(), &setting)
                .await
                .unwrap();

            let _ = dbg!(&res);

            for line in res.lines() {
                println!("{}. {}", line.0, line.1);
            }
        });
    }
}

//! 번역 작업을 위한 OpenAI API 프롬프트 추상화

use async_openai::{
    error::OpenAIError,
    types::{ChatCompletionRequestMessage, CreateChatCompletionRequest, Role},
};
use compact_str::CompactString;
use default::default;

use crate::lang::Language;

static_assertions::assert_impl_all!(Instance: Send, Sync, Unpin);

/// OpenAI API 호출 관리
#[derive(Debug, Clone)]
pub struct Instance {
    ai: async_openai::Client,
}

#[derive(Debug, derive_more::Display)]
pub enum ChatModel {
    #[display(fmt = "gpt-3.5-turbo")]
    Gpt3_5,
    #[display(fmt = "gpt-4")]
    Gpt4,
}

/// 번역에 필요한 정적 세팅(모델 정보, 토큰 길이 등)을 포함합니다.
#[derive(Debug, typed_builder::TypedBuilder)]
pub struct Settings {
    source_lang: Language,
    dest_lang: Language,
    model: ChatModel,
}

/* ------------------------------------------ Init Ops ------------------------------------------ */
impl Instance {
    pub fn new(api_key: &str) -> Self {
        Self {
            ai: async_openai::Client::new().with_api_key(api_key),
        }
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

#[derive(thiserror::Error, Debug)]
pub enum ProperNounRetrievalError {
    #[error("OpenAI API error: {0}")]
    OpenAI(#[from] OpenAIError),

    #[error("No choice found")]
    NoChoice,

    #[error("The translation profile failed to parse output message")]
    NounListParseFailed,

    #[error("Token length limit exceeded.")]
    TokenLengthLimit,
}

impl Instance {
    pub async fn retrieve_proper_nouns(
        &self,
        content: &str,
        opt: &Settings,
    ) -> Result<ProperNounRetrievalResult, ProperNounRetrievalError> {
        let mut req = CreateChatCompletionRequest::default();
        req.model = opt.model.to_string();

        // 프롬프트를 빌드한다
        let msg = |role: Role, msg: &str| ChatCompletionRequestMessage {
            role,
            content: msg.into(),
            ..default()
        };

        req.messages.push(msg(
            Role::System,
            opt.dest_lang
                .profile()
                .proper_noun_instruction(opt.source_lang),
        ));

        req.messages.push(msg(Role::User, content));
        let rep = self.ai.chat().create(req).await?;
        let msg = rep
            .choices
            .first()
            .ok_or(ProperNounRetrievalError::NoChoice)?;

        if let Some(true) = msg.finish_reason.as_ref().map(|x| x == "length") {
            return Err(ProperNounRetrievalError::TokenLengthLimit);
        }

        let nouns = opt
            .dest_lang
            .profile()
            .parse_proper_noun_output(opt.source_lang, &msg.message.content)
            .ok_or(ProperNounRetrievalError::NounListParseFailed)?;

        let (n_prompt, n_compl) = rep
            .usage
            .map(|x| (x.prompt_tokens, x.completion_tokens))
            .ok_or(ProperNounRetrievalError::NoChoice)?;

        Ok(ProperNounRetrievalResult {
            nouns,
            num_prompt_tokens: n_prompt as _,
            num_compl_tokens: n_compl as _,
            _no_build: (),
        })
    }
}

/* --------------------------------------- Translation Ops -------------------------------------- */

/* ---------------------------------------------------------------------------------------------- */
/*                                          TEST SECTION                                          */
/* ---------------------------------------------------------------------------------------------- */
#[cfg(test)]
mod __test {
    use std::future::Future;

    use crate::lang::Language;

    use super::Settings;

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
        dbg!(&sample_path);

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
    fn ping_api() {
        let Some(content) = try_find_jp_sample(0..10) else { return };

        exec_test(|h| async move {
            let setting = Settings::builder()
                .source_lang(Language::Japanese)
                .dest_lang(Language::Korean)
                .model(super::ChatModel::Gpt3_5)
                .build();

            let res = h.retrieve_proper_nouns(&content, &setting).await;
            let _ = dbg!("res: {:#?}", res);
        });
    }
}

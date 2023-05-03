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

pub struct ProperNounRetrievalResult {
    pub nouns: Vec<CompactString>,
    pub num_prompt_tokens: usize,
    pub num_compl_tokens: usize,

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
            opt.source_lang.profile().proper_noun_instruction(),
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
            .source_lang
            .profile()
            .parse_proper_noun_output(&msg.message.content)
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

    #[test]
    fn ping_api() {
        exec_test(|h| async {});
    }
}

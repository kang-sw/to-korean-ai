use compact_str::CompactString;
use std::borrow::Cow;

/// 각종 시스템 프롬프트를 생성하기 위한 프로필입니다.
pub trait PromptProfile: Send + Sync + std::fmt::Debug {
    fn proper_noun_instruction(&self, src_lang: Language) -> &str;

    /// 서버 응답을 파싱해서 고유명사 리스트 추출. Parse 실패 시 None
    fn parse_proper_noun_output(
        &self,
        src_lang: Language,
        response: &str,
    ) -> Option<Vec<CompactString>>;

    /// 입력으로부터 등장 인물 사이의 관계를 파악하고, 인물 사이의 대화에서 분위기 등을 조정하기
    /// 위해 활용합니다.
    ///
    /// - `previous_note`: 앞서 정리된 출력입니다. 관계도는 이를 바탕으로 업데이트됩니다.
    fn query_relationship(&self, src_lang: Language, prev_note: Option<&str>) -> Cow<str>;

    /// 번역 단계에서 제공: 적절한 번역 지시 사항을 지정합니다. 핵심이 되는 프롬프트입니다.
    fn trans_instruction(&self, src_lang: Language) -> Cow<str>;

    /// 번역 단계에서 제공: 고유 명사 사전에 대한 시스템 안내 프롬프트를 생성합니다.
    fn trans_appendix_proper_noun_dict(
        &self,
        dest_lang: Language,
        dict: &[(&str, &str)],
    ) -> Cow<str>;

    /// 번역 단계에서 제공: 인물 사이의 관계 서술에 대한 시스템 안내 프롬프트입니다.
    fn trans_appendix_relationship(&self, src_lang: Language, desc: &str) -> Cow<str>;
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, Copy, Default)]
pub enum Language {
    #[default]
    English,

    Korean,

    Japanese,
}

impl Language {
    pub fn to_english(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Korean => "Korean",
            Language::Japanese => "Japanese",
        }
    }

    pub fn to_korean(&self) -> &'static str {
        match self {
            Language::English => "영어",
            Language::Korean => "한국어",
            Language::Japanese => "일본어",
        }
    }
}

pub mod profiles {
    #[derive(Debug)]
    pub struct KoreanV1;

    mod korean_v1 {
        use std::borrow::Cow;

        use compact_str::CompactString;
        use default::default;
        use indoc::indoc;
        use lazy_static::lazy_static;

        use super::KoreanV1;
        use crate::lang::Language;

        macro_rules! static_lang_str_ko {
            ($str:expr, $source_lang:expr) => {{
                const FMT_BASE: &str = indoc::indoc!($str);

                lazy_static! {
                    static ref JP: String = FMT_BASE.replace("{{LANG}}", "일본어");
                    static ref EN: String = FMT_BASE.replace("{{LANG}}", "영어");
                };

                match $source_lang {
                    Language::English => &EN,
                    Language::Japanese => &JP,
                    Language::Korean => panic!("Korean -> Korean not allowed!"),
                }
            }};
        }

        impl super::super::PromptProfile for KoreanV1 {
            fn proper_noun_instruction(&self, source_lang: Language) -> &str {
                static_lang_str_ko!(
                    r##"    제시된 {{LANG}} 원문으로부터,
                 
                            - "사람 이름"으로 추론되는 모든 단어를 추출하여 `person` 배열에 나열하십시오.
                            - "고유 명사 지명"으로 추론되는 모든 단어를 추출하여 `location` 배열에 나열하십시오.
                    
                            출력 형식은 YAML입니다."##,
                    source_lang
                )
            }

            fn parse_proper_noun_output(
                &self,
                _source_lang: Language,
                response: &str,
            ) -> Option<Vec<compact_str::CompactString>> {
                log::debug!("parsing noun output: {:?}", response);

                // 마크다운 문법으로 YAML 받음 ... 먼저 앞뒤의 ```yaml ~~ ``` 떼어낸다.
                let lines: Vec<_> = response
                    .lines()
                    .skip_while(|line| !line.starts_with("```yaml"))
                    .collect();

                let close_pos = lines
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, line)| line.starts_with("```"))
                    .map(|(pos, _)| pos)?;

                let lines = &lines[..close_pos];

                #[derive(serde::Deserialize)]
                struct Output<'a> {
                    #[serde(borrow)]
                    person: Vec<&'a str>,
                    #[serde(borrow)]
                    location: Vec<&'a str>,
                }

                let rebuilt_text = lines.join("\n");
                let output: Output<'_> = serde_yaml::from_str(&rebuilt_text).ok()?;

                Some(
                    output
                        .person
                        .into_iter()
                        .chain(output.location.into_iter())
                        .map(|x| CompactString::new(x))
                        .collect(),
                )
            }

            fn trans_instruction(&self, source_lang: Language) -> Cow<str> {
                match source_lang {
                Language::English => todo!(),
                Language::Korean => panic!("Korean -> Korean not allowed!"),
                Language::Japanese => indoc!(
                    r##"    # 지시사항
                    
                            입력되는 일본어 문장을 한국어로 번역한다. 다음은 그 요구 조건이다.
        
                            * 제공되는 모든 문장은 연결된 내용이므로, 입력된 원문의 맥락을 적극적으로 고려한다.
                            * (가능하다면) 일본어 관용구는 적절하게 대응되는 한국어 관용구로 가공한다.
                            * 올바른 한국어 문장으로 가공한다.
                            * 등장인물의 대사를 포함한 모든 문장에서, 존댓말의 사용을 최소화한다."##
                ),
            }.into()
            }

            fn query_relationship(
                &self,
                source_lang: Language,
                previous_note: Option<&str>,
            ) -> Cow<str> {
                // TODO: previous_note 유/무에 따라 다른 프롬프트 제공. previous_note 있으면
                // 참고하여 관계도를 업데이트하도록 유도

                /*

                **지시사항**

                - 제공된 일본어 원문으로부터, 등장인물 사이의 관계를 그래프 형태로 작성한다.
                - 중요도가 낮아 보이는 인물은 무시한다.

                (도중에 맥락에 무관하게 장면이 변경될 수 있음)

                **출력 형식**

                - 등장인물 -> 다른등장인물: 상대에 대한 감정 및 사회적인 관계

                (예시)
                A -> B: 대등한 친구이며, 친애한다.
                B -> A: 대등한 친구이지만, 죄의식을 갖고 있다.
                C -> A: 무관한 사람이지만, 적대시한다.

                ---

                각 관계가 중복되지 않도록 주의하십시오.

                지금부터 일본어 원문을 입력할 것입니다. 준비되었습니까?

                */

                default()
            }

            fn trans_appendix_proper_noun_dict(
                &self,
                _src_lang: Language,
                dict: &[(&str, &str)],
            ) -> Cow<str> {
                if dict.is_empty() {
                    return default();
                }

                let mut buf = String::with_capacity(512);

                #[allow(unused_must_use)]
                {
                    let b: &mut dyn std::fmt::Write = &mut buf;
                    writeln!(b, "다음은 고유 명사의 번역을 제안하는 사전입니다:");
                    writeln!(b, "");
                    writeln!(b, "[시작]");

                    for (src, dest) in dict {
                        writeln!(b, "* {} -> {}", src, dest);
                    }

                    writeln!(b, "[끝]");
                }

                buf.into()
            }

            fn trans_appendix_relationship(&self, src_lang: Language, desc: &str) -> Cow<str> {
                if desc.is_empty() {
                    return default();
                }

                // TODO: 다음은 인물 사이의 관계를 정리한 노트입니다. 참고하여 번역하십시오

                default()
            }
        }
    }
}

use std::collections::HashSet;

use genai::chat::{ChatMessage, ChatRequest, ChatResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Error, Result, Wizard};
use crate::core::config::transformer::{FieldLocation, RenameFields};
use crate::core::config::{Config, Resolver};
use crate::core::valid::{Valid, Validator};
use crate::core::{AsyncTransform, Mustache, Transform};

const BASE_TEMPLATE: &str = include_str!("prompts/infer_field_name.md");

pub struct InferFieldName {
    wizard: Wizard<Question, Answer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Answer {
    suggestions: Vec<String>,
}

impl TryFrom<ChatResponse> for Answer {
    type Error = Error;

    fn try_from(response: ChatResponse) -> Result<Self> {
        let message_content = response.content.ok_or(Error::EmptyResponse)?;
        let text_content = message_content.text_as_str().ok_or(Error::EmptyResponse)?;
        Ok(serde_json::from_str(text_content)?)
    }
}

#[derive(Clone, Serialize)]
struct Question {
    url: String,
    method: String,
}

impl TryInto<ChatRequest> for Question {
    type Error = Error;

    fn try_into(self) -> Result<ChatRequest> {
        let template = Mustache::parse(BASE_TEMPLATE);

        let input1 = Question {
            url: "https://jsonplaceholder.typicode.com/posts".into(),
            method: "GET".into(),
        };

        let output1 = Answer {
            suggestions: vec![
                "posts".into(),
                "postList".into(),
                "articles".into(),
                "articlesList".into(),
                "entries".into(),
            ],
        };

        let input2 = Question {
            url: "https://jsonplaceholder.typicode.com/posts".into(),
            method: "POST".into(),
        };

        let output2 = Answer {
            suggestions: vec![
                "createPost".into(),
                "createArticle".into(),
                "createEntry".into(),
                "createNewPost".into(),
                "createNewArticle".into(),
            ],
        };

        let input3 = Question {
            url: "https://jsonplaceholder.typicode.com/posts/1".into(),
            method: "DELETE".into(),
        };

        let output3 = Answer {
            suggestions: vec![
                "deletePost".into(),
                "removePost".into(),
                "removePostById".into(),
                "deleteEntry".into(),
                "deleteEntryById".into(),
            ],
        };

        let context = json!({
            "input1": input1,
            "input2": input2,
            "input3": input3,
            "output1": output1,
            "output2": output2,
            "output3": output3,
            "count": 5,
        });

        let rendered_prompt = template.render(&context);

        Ok(ChatRequest::new(vec![
            ChatMessage::system(rendered_prompt),
            ChatMessage::user(serde_json::to_string(&self)?),
        ]))
    }
}

impl InferFieldName {
    pub fn new(model: String, secret: Option<String>) -> InferFieldName {
        Self { wizard: Wizard::new(model, secret) }
    }

    pub async fn generate(&self, config: &Config) -> Result<Vec<(String, FieldLocation)>> {
        let mut mapping: Vec<(String, FieldLocation)> = Vec::new();

        for (type_name, type_) in config.types.iter() {
            let mut visited_types = HashSet::new();
            for (field_name, field) in type_.fields.iter() {
                if field.resolver.is_none() {
                    continue;
                }
                let question = match &field.resolver {
                    Some(Resolver::Http(http)) => {
                        let base_url = http
                            .base_url
                            .clone()
                            .or_else(|| {
                                config
                                    .upstream
                                    .base_url
                                    .as_ref()
                                    .map(|base| base.to_owned() + &http.path)
                            })
                            .ok_or_else(|| {
                                Error::Err("Base URL is required for HTTP resolver".into())
                            })?;

                        Question { url: base_url, method: http.method.to_string() }
                    }
                    Some(Resolver::Grpc(grpc)) => {
                        let base_url = grpc
                            .base_url
                            .as_ref()
                            .or(config.upstream.base_url.as_ref())
                            .map(String::as_str)
                            .ok_or_else(|| {
                                Error::Err("Base URL is required for gRPC resolver".into())
                            })?;

                        Question { url: base_url.to_owned(), method: grpc.method.to_string() }
                    }
                    _ => {
                        unreachable!("Unsupported for other resolvers");
                    }
                };

                let mut delay = 3;
                loop {
                    let answer = self.wizard.ask(question.clone()).await;
                    match answer {
                        Ok(answer) => {
                            tracing::info!(
                                "Suggestions for Field {}: [{:?}]",
                                field_name,
                                answer.suggestions,
                            );
                            let new_field_name =
                                answer.suggestions.into_iter().find(|field_name| {
                                    !type_.fields.contains_key(field_name)
                                        && !visited_types.contains(field_name)
                                });

                            if let Some(new_field_name) = new_field_name {
                                visited_types.insert(new_field_name.clone());
                                mapping.push((
                                    field_name.to_owned(),
                                    FieldLocation {
                                        new_field_name,
                                        type_name: type_name.to_owned(),
                                    },
                                ));
                            }

                            break;
                        }
                        Err(e) => {
                            if let Error::GenAI(_) = e {
                                tracing::warn!(
                                    "Unable to retrieve a name for the field '{}'. Retrying in {}s",
                                    field_name,
                                    delay
                                );
                                tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                                delay *= std::cmp::min(delay * 2, 60);
                            }
                        }
                    }
                }
            }
        }

        Ok(mapping)
    }
}

impl AsyncTransform for InferFieldName {
    type Value = Config;
    type Error = Error;

    async fn transform(&self, value: Self::Value) -> Valid<Self::Value, Self::Error> {
        match self.generate(&value).await {
            Ok(suggested_names) => {
                match RenameFields::new(suggested_names)
                    .transform(value)
                    .to_result()
                {
                    Ok(v) => Valid::succeed(v),
                    Err(e) => Valid::fail(Error::Err(e.to_string())),
                }
            }
            Err(err) => Valid::fail(err),
        }
    }
}

#[cfg(test)]
mod test {
    use genai::chat::{ChatRequest, ChatResponse, MessageContent};

    use super::{Answer, Question};

    #[test]
    fn test_to_chat_request_conversion() {
        let question = Question {
            url: "https://jsonplaceholder.typicode.com/posts".to_string(),
            method: "GET".to_string(),
        };
        let request: ChatRequest = question.try_into().unwrap();
        insta::assert_debug_snapshot!(request);
    }

    #[test]
    fn test_chat_response_parse() {
        let resp = ChatResponse {
            content: Some(MessageContent::Text(
                "{\"suggestions\":[\"posts\",\"postList\",\"articles\",\"articlesList\",\"entries\"]}".to_owned(),
            )),
            ..Default::default()
        };
        let answer = Answer::try_from(resp).unwrap();
        insta::assert_debug_snapshot!(answer);
    }
}
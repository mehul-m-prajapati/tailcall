use std::fmt::Formatter;
use std::marker::PhantomData;

use serde;
use serde::de;

#[derive(Debug)]
pub struct Post {
    pub user_id: u64,
    pub id: u64,
    pub title: String,
    pub body: String,
}

// IMPORTANT: Do not delete this implementation.
// This is the implementation of the Deserialize trait for the Post struct.
// It was auto-generated by the derive macro and around 4x faster than the
// deserializer of serde_json::Value.
impl<'de> serde::Deserialize<'de> for Post {
    fn deserialize<D>(deserializer: D) -> serde::__private::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Field {
            UserId,
            Id,
            Title,
            Body,
            Unknown,
        }

        struct FieldVisitor;

        impl<'de> de::Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, formatter: &mut Formatter) -> serde::__private::fmt::Result {
                formatter.write_str("field identifier")
            }

            fn visit_str<E>(self, value: &str) -> serde::__private::Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "user_id" => Ok(Field::UserId),
                    "id" => Ok(Field::Id),
                    "title" => Ok(Field::Title),
                    "body" => Ok(Field::Body),
                    _ => Ok(Field::Unknown),
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> serde::__private::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct Visitor<'de> {
            marker: PhantomData<Post>,
            lifetime: PhantomData<&'de ()>,
        }

        impl<'de> de::Visitor<'de> for Visitor<'de> {
            type Value = Post;

            fn expecting(&self, formatter: &mut Formatter) -> serde::__private::fmt::Result {
                formatter.write_str("struct Post")
            }

            fn visit_map<A>(self, mut map: A) -> serde::__private::Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut user_id = None;
                let mut id = None;
                let mut title = None;
                let mut body = None;

                while let Some(key) = map.next_key::<Field>()? {
                    match key {
                        Field::UserId => {
                            if user_id.is_some() {
                                return Err(de::Error::duplicate_field("user_id"));
                            }
                            user_id = Some(map.next_value()?);
                        }
                        Field::Id => {
                            if id.is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Title => {
                            if title.is_some() {
                                return Err(de::Error::duplicate_field("title"));
                            }
                            title = Some(map.next_value()?);
                        }
                        Field::Body => {
                            if body.is_some() {
                                return Err(de::Error::duplicate_field("body"));
                            }
                            body = Some(map.next_value()?);
                        }
                        Field::Unknown => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let user_id = user_id.ok_or_else(|| de::Error::missing_field("user_id"))?;
                let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                let title = title.ok_or_else(|| de::Error::missing_field("title"))?;
                let body = body.ok_or_else(|| de::Error::missing_field("body"))?;

                Ok(Post { user_id, id, title, body })
            }
        }

        const FIELDS: &[&str] = &["user_id", "id", "title", "body"];

        deserializer.deserialize_struct(
            "Post",
            FIELDS,
            Visitor { marker: PhantomData, lifetime: PhantomData },
        )
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_posts() {
        const JSON: &str = include_str!("../data/posts.json");
        let posts: Vec<super::Post> = serde_json::from_str(JSON).unwrap();

        assert_eq!(posts.len(), 100);
    }
}
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use ollama_rs::{
    Ollama,
    error::OllamaError,
    generation::{
        chat::{ChatMessage, request::ChatMessageRequest},
        completion::request::GenerationRequest,
        embeddings::{
            GenerateEmbeddingsResponse,
            request::{EmbeddingsInput, GenerateEmbeddingsRequest},
        },
    },
};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;

use tokio::io::{self, AsyncWriteExt};
use tokio_stream::StreamExt;

#[derive(Deserialize, Serialize, Debug, sqlx::FromRow)]
struct AlquranAyat {
    id: i32,
    id_surah: i32,
    surah: String,
    ayat: i32,
    tr: String,
    idn: String,
    en: String,
    tafsir: String,
}
const TEXT_EMBED_MODEL_NAME: &str = "nomic-embed-text";
const CHAT_MODEL_NAME: &str = "deepseek-r1:14b";

async fn get_embedding_from_ayat(
    embed: &AlquranAyat,
) -> Result<GenerateEmbeddingsResponse, OllamaError> {
    let ollama = Ollama::default();
    // let input = EmbeddingsInput::Single(
    //     format!(
    //         "surah: {}, ayat: {}, tr: {}, idn: {}, tafsir: {}",
    //         embed.surah, embed.ayat, embed.tr, embed.idn, embed.tafsir
    //     )
    //     .to_lowercase(),
    // );
    let input =
        EmbeddingsInput::Single(format!("search_document: artinya {}", embed.en).to_lowercase());
    println!("===============");
    println!("generated embeding for input {:?}", &input);
    println!("===============");

    let request: GenerateEmbeddingsRequest =
        GenerateEmbeddingsRequest::new(TEXT_EMBED_MODEL_NAME.to_string(), input);
    let res = ollama.generate_embeddings(request).await.unwrap();
    Ok(res)
}

async fn convert_table_alquran_to_vector() -> Result<(), Box<dyn std::error::Error>> {
    println!("RUNNING embedding from db");
    let pool = PgPoolOptions::new()
        .connect("postgres://alquran:alquran@127.0.0.1/alquran?currentSchema=alquran") // Update with your database credentials
        .await?;

    // // Fetch all Ayat
    let rows: Vec<AlquranAyat> = sqlx::query_as(
        "select 
        a.id
        , a.surah as id_surah
        , s.nama_latin as surah
        , a.ayat
        , a.tr
        , a.idn
        , a.en
        , a.tafsir
        from alquran_ayat a 
        join alquran_surah s on s.id = a.surah
        order by a.surah, a.ayat
        ",
    )
    .fetch_all(&pool)
    .await
    .expect("Error fetching ayat");

    for embed in rows {
        println!(
            "get embedding for surah {}, ayat {}",
            embed.surah, embed.ayat
        );
        match get_embedding_from_ayat(&embed).await {
            Ok(embedding) => {
                print!("vector len {} ", embedding.embeddings.len());
                print!(
                    "vector len at 1 {} ",
                    embedding.embeddings.get(0).unwrap().len()
                );
                // Update the embedding in the database
                sqlx::query("UPDATE alquran.alquran_ayat SET embedding = $1 WHERE id = $2")
                    .bind(&embedding.embeddings.get(0))
                    .bind(embed.id)
                    .execute(&pool)
                    .await
                    .expect("Error updating Table");
            }
            Err(e) => {
                eprintln!(
                    "Error getting embedding for surah {}, ayat {}, {}",
                    embed.surah, embed.ayat, e
                );
            }
        }
    }
    Ok(())
}

async fn search_db(
    search: &String,
) -> Result<Vec<ResultQueryAlquranAyat>, Box<dyn std::error::Error>> {
    println!("RUNNING EMBEDDING QUERY");
    let pool = PgPoolOptions::new()
        .connect("postgres://alquran:alquran@127.0.0.1/alquran?currentSchema=alquran") // Update with your database credentials
        .await?;
    let query = format!("search_query: {}", search).to_string();

    let vector_res = get_vector_from_query(&query).await;

    match vector_res {
        Ok(vector) => {
            print!("vector len {}", vector.embeddings.len());
            println!("Returning query result");
            let rows: Vec<ResultQueryAlquranAyat> = sqlx::query_as(
                "select 
                a.id
                , a.surah
                , s.nama_latin as surah_name
                , a.ayat
                , a.tr
                , a.idn
                , a.en
                , a.tafsir
                , 1 - ( a.embedding <=> $1::vector) as distance
                from alquran_ayat a 
                join alquran_surah s on s.id = a.surah
                order by 1-(a.embedding <=> $1::vector) desc
                limit 5 
                ",
            )
            .bind(vector.embeddings.get(0))
            .fetch_all(&pool)
            .await
            .expect("RETURN FAILED");

            for (index, row) in rows.iter().enumerate() {
                // println!("RESULT {:?}", row);
                println!("=====================");
                println!("QUERY {}", query);
                println!("=====================");
                println!("rank {} ===========================================", index);
                println!("distance {:?}", row.distance);
                println!(
                    "res surah: {:?} verse :{:?} \ntranslation: {:?}",
                    row.surah_name, row.ayat, row.idn
                );
                println!("\nTafsir {:?}", row.tafsir);
                println!("===========================================\n");
            }
            Ok(rows)
        }
        Err(e) => {
            println!("Error Query {:?}", e);
            Err(Box::new(e))
        }
    }
}

#[derive(Deserialize, Serialize, Debug, sqlx::FromRow)]
struct ResultQueryAlquranAyat {
    id: i32,
    surah: i32,
    surah_name: String,
    ayat: i32,
    tr: String,
    idn: String,
    en: String,
    tafsir: String,
    distance: f64,
}
async fn get_vector_from_query(query: &String) -> Result<GenerateEmbeddingsResponse, OllamaError> {
    let ollama: Ollama = Ollama::default();

    let request: GenerateEmbeddingsRequest = GenerateEmbeddingsRequest::new(
        TEXT_EMBED_MODEL_NAME.to_string(),
        EmbeddingsInput::Single(format!("{}", query).to_lowercase()),
    );
    let res = ollama.generate_embeddings(request).await.unwrap();

    Ok(res)
}

async fn genrate_response(query: &String, sim_res: Vec<ResultQueryAlquranAyat>) {
    let ollama: Ollama = Ollama::default();
    let remapped_context : Vec<String> = sim_res.iter().map(|f| {
        format!("source surah: {}, source ayat: {}, translation in english: {} , translation in indonesia: {}, tafsir in indonesi: {}",f.surah,f.ayat,f.en,f.idn, f.tafsir)}
    ).collect();
    let request: ChatMessageRequest = ChatMessageRequest::new(
        CHAT_MODEL_NAME.to_string(),
        vec![
            ChatMessage::user(
                format!(
                    "you are an agent that help people understand about islam. 
                    strictly use context of islam as religion.
                    here is some context {:?}
                    strictly use only context above ignore other source.
                    ",
                    remapped_context
                )
                .to_string(),
            ),
            ChatMessage::user(format!("{}", query).to_string()),
        ],
    );
    println!("GENERATING RESPONSE WITH INPUT {:?}", request);
    let mut stream = ollama.send_chat_messages_stream(request).await.unwrap();

    let mut stdout = std::io::stdout();
    let mut final_response = String::new();
    while let Some(res) = stream.next().await {
        let responses = res.unwrap();
        stdout
            .write_all(responses.message.content.as_bytes())
            .unwrap();
        stdout.flush().unwrap();
        final_response += responses.message.content.as_str();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RUNNING THIS APP WITH MODEL {}", TEXT_EMBED_MODEL_NAME);
    #[cfg(feature = "run_embeding_model")]
    {
        let _ = convert_table_alquran_to_vector().await;
    }

    #[cfg(not(feature = "run_embeding_model"))]
    {
        let search = "explain about animal sacrefice".to_string();
        let res = search_db(&search).await;
        let sim_res: Vec<ResultQueryAlquranAyat> = res.unwrap();
        genrate_response(&search, sim_res).await;
    }
    // Set up the database connection pool

    Ok(())
}

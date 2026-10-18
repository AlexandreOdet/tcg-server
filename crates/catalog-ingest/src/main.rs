// TODO(phase 1) : binaire d'ingestion du catalogue.
//  - lire la liste d'articles populaires, récupérer résumé (API REST Wikipédia) et vues (Pageviews)
//  - limiter la concurrence (`Semaphore` ou `buffer_unordered`), User-Agent identifiable
//  - upsert sur `wiki_page_id` pour pouvoir reprendre après interruption
//  - calculer la rareté avec la fonction pure de `domain`

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("catalog-ingest : pas encore implémenté (phase 1)");
    Ok(())
}

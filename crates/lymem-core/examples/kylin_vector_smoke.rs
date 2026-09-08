#[cfg(not(feature = "kylin-vector"))]
fn main() {
    eprintln!("build with --features kylin-vector");
    std::process::exit(2);
}

#[cfg(feature = "kylin-vector")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use lymem_core::embedding::HashEmbedder;
    use lymem_core::model::{MemoryKind, MemoryRecord, Tier};
    use lymem_core::retrieval::SearchParams;
    use lymem_core::MemoryStore;

    std::env::set_var("LYMEM_VECTOR_BACKEND", "kylin");
    std::env::set_var("LYMEM_KYLIN_VECTOR_COLLECTION", "lymem_sdk_smoke_v1");
    let root = std::env::temp_dir().join(format!("lymem-kylin-vector-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = MemoryStore::open(
        &root.join("smoke.db"),
        &root.join("fts"),
        Box::new(HashEmbedder::new(64)),
    )?;
    let mut record = MemoryRecord::new(
        Tier::Knowledge,
        MemoryKind::Fact,
        "SDK smoke",
        "麒麟向量数据库 SDK 真实调用验证：办公文档默认导出 PDF。",
    );
    record.scene = "sdk-smoke".into();
    let id = store.put(record)?;
    let mut params = SearchParams::new("办公文档导出格式");
    params.top_k = 3;
    params.use_bm25 = false;
    params.use_graph = false;
    let hits = store.search(&params)?;
    if !hits.iter().any(|hit| hit.record.id == id) {
        return Err("Kylin vector search did not return inserted memory".into());
    }
    store.purge(&[id])?;
    println!(
        "vector_backend={} inserted={} searched={} purged=1",
        store.vector_backend_name(),
        id,
        hits.len()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

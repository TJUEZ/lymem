# lymem vs mem0: Kylin LoCoMo retrieval benchmark

Date: 2026-09-08

## Protocol

- Dataset: LoCoMo `conv-26`, 199 questions.
- Embedding: Kylin GTE, 768 dimensions.
- Vector backend: Kylin Vector Engine for both systems.
- Metric: whether at least one gold evidence turn is represented in the first K results.
- lymem indexes raw and three-turn window views. mem0 uses its existing LLM-extracted memories migrated from Qdrant into the Kylin backend.
- The comparison is retrieval-only. It does not include answer generation or an LLM judge.

## Results

| System | K | Hit@K | Evidence recall | MRR | p50 | p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| lymem default | 5 | 40.70% | 22.24% | 17.89% | 39.61 ms | 59.53 ms |
| lymem adaptive | 5 | **41.71%** | 26.88% | **21.86%** | 25.80 ms | 51.72 ms |
| mem0 | 5 | 35.68% | **34.34%** | n/a | 1.98 ms* | 2.75 ms* |
| lymem adaptive | 10 | **54.27%** | 38.38% | 16.98% | 36.23 ms | 62.02 ms |
| mem0 | 10 | 50.25% | **46.15%** | n/a | 1.62 ms* | 2.08 ms* |

At K=5, adaptive lymem leads mem0 by 6.03 percentage points in Hit@5. At K=10, it leads by 4.02 points.

`*` The latency paths are not equivalent. mem0 calls a warm HTTP embedding proxy and then the Kylin index. `lymem-bench` calls the Kylin embedder client directly and includes that call plus BM25, graph expansion, SQLite filtering, fusion and touch updates. These values describe each current end-to-end retrieval path, not isolated vector database latency.

## Adaptive A/B

The opt-in `LYMEM_ADAPTIVE_RETRIEVAL=1` policy changed Hit@5 from 40.70% to 41.71%. It produced 14 newly correct questions and regressed 12 previously correct questions.

| LoCoMo category | Default Hit@5 | Adaptive Hit@5 | mem0 Hit@5 |
| --- | ---: | ---: | ---: |
| 1 | 9.38% | 12.50% | **15.62%** |
| 2 | 54.05% | **67.57%** | 54.05% |
| 3 | 7.69% | 15.38% | **23.08%** |
| 4 | 48.57% | 40.00% | **51.43%** |
| 5 | 48.94% | **51.06%** | 14.89% |

The first heuristic improves multi-hop and temporal retrieval but harms open-domain questions. It should remain opt-in until the router uses calibrated confidence and falls back to default fusion for ambiguous queries.

## Storage and operational notes

- lymem benchmark data for this conversation: about 7.37 MiB, including SQLite metadata/vector fallback and Tantivy files.
- mem0 payload sidecar: about 0.96 MiB. This excludes the Kylin process memory and the original Qdrant source storage, so it is not a complete footprint comparison.
- The current mem0 Kylin adapter stores payloads in SQLite but the Kylin vector index is process-local. A fresh process must reload the 1,380 vectors before search; `--skip-migrate` is valid only while the same index process remains alive.

## Next experiments

1. Add query-router confidence and default-fusion fallback, then repeat the paired A/B test.
2. Measure embedding, vector search, BM25, graph, fusion and write-back latency separately.
3. Add Kylin ANN oversampling followed by exact cosine reranking.
4. Run all ten LoCoMo conversations after the retrieval policy is stable.
5. Run answer-generation accuracy with one shared LLM and judge protocol.

Raw result files are stored outside the Git repository under `benchmark/results/experiments/` in the workspace root.

CREATE TABLE IF NOT EXISTS receipts (
  id TEXT PRIMARY KEY, ts INTEGER NOT NULL, source TEXT NOT NULL, class_hash TEXT NOT NULL, install_id TEXT,
  platform TEXT, arch TEXT, gpu TEXT, threads INTEGER, memory_gb REAL, memory_known INTEGER,
  bandwidth_gbps REAL NOT NULL, cpu_bandwidth_gbps REAL, gpu_bandwidth_gbps REAL, bandwidth_source TEXT,
  gflops_f32 REAL, k REAL, extra TEXT, rl_day TEXT, rl_hash TEXT);
CREATE INDEX IF NOT EXISTS receipts_class ON receipts(class_hash);
CREATE INDEX IF NOT EXISTS receipts_install ON receipts(install_id);
CREATE INDEX IF NOT EXISTS receipts_rl ON receipts(rl_day, rl_hash);
CREATE TABLE IF NOT EXISTS runs (
  receipt_id TEXT NOT NULL, class_hash TEXT NOT NULL, install_id TEXT, model TEXT NOT NULL, file_gb REAL, tps REAL, predicted_tps REAL, threads INTEGER, ttft_s REAL);
CREATE INDEX IF NOT EXISTS runs_class ON runs(class_hash, model);
CREATE INDEX IF NOT EXISTS runs_install ON runs(install_id);
-- generic per-day rate-limit counters for endpoints that don't insert a row of their own (e.g. /v1/delete)
CREATE TABLE IF NOT EXISTS actions (day TEXT NOT NULL, rl_hash TEXT NOT NULL, endpoint TEXT NOT NULL, n INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (day, rl_hash, endpoint));

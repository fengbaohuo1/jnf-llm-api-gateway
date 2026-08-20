-- 0002 渠道增加故障转移排序字段：priority（优先级，越大越先） + weight（组内权重，越大越可能先被选中）
ALTER TABLE channels ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN weight INTEGER NOT NULL DEFAULT 1;

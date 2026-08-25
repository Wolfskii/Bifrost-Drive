ALTER TABLE connections
ADD COLUMN configuration_json TEXT NOT NULL DEFAULT '{}';
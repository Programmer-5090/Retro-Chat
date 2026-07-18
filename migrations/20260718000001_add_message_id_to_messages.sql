ALTER TABLE messages ADD COLUMN message_id VARCHAR(12);

UPDATE messages SET message_id = LEFT(md5(random()::text), 12);

ALTER TABLE messages ALTER COLUMN message_id SET NOT NULL;

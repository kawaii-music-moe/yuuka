-- NULL means inherit deployment defaults. Existing API keys are unchanged.
ALTER TABLE bots ADD COLUMN llm_provider TEXT CHECK (llm_provider IS NULL OR llm_provider = 'gemini');
ALTER TABLE bots ADD COLUMN llm_model TEXT;

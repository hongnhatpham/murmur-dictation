# Rotate hosted STT before local processing

Finalized speech recognition tries Deepgram, Groq, and AssemblyAI before local Whisper, rotating only after quota, authentication, service, or repeated timeout failures. Murmur keeps AssemblyAI as the live-stream fallback because Groq Whisper accepts completed audio rather than a live stream. Supporting several hosted providers costs more integration work, but it extends free usage and preserves hosted speed before accepting the local model's resource use and weaker diarization.

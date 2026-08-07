# Configuration

Phi resolves one typed configuration snapshot before constructing an Agent.
Create a default Home and edit its YAML configuration:

```bash
phi home new .phi
```

```yaml
model:
  name: gpt-5
  temperature: 1.0
  max_tokens: 32000
  reasoning:
    enabled: true
    effort: medium
    token_budget: 4096
provider:
  kind: openai_chat
  api_base: https://api.openai.com/v1
  api_key: your_api_key
runtime:
  # Omit system to use Phi's built-in prompt; set it to "" to disable it.
  system: ""
  max_steps: 1000000
  context_tokens: 262144
executor:
  tool_threshold_tokens: 6144
  tool_preview_bytes: 2048
tools: []
```

Agent, Doctor, and Session commands that construct provider state accept an
explicit replacement file:

```bash
phi run --config ./config.yml --user "Inspect this repository"
phi doctor --config ./config.yml
phi session new work.session --config ./config.yml
```

The precedence is built-in defaults, then either the Home configuration or the
`--config` replacement, then `PHI_*` environment variables. Home YAML and the
explicit file are alternatives; they are not merged with each other.

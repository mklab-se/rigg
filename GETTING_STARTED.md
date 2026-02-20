# Getting Started: Build an Agentic RAG System

This walkthrough takes you from zero to a deployed Agentic RAG system using hoist, Azure AI Search, and Microsoft Foundry.

## How the Pieces Connect

An Agentic RAG system has four layers, managed across two Azure services:

```
Microsoft Foundry                    Azure AI Search
─────────────────                    ────────────────
Agent                                Knowledge Base
  └─ tools: [mcp → KB]        ───►    └─ Knowledge Source
                                           └─ Index (auto-provisioned)
                                              Indexer (auto-provisioned)
                                              Data Source (auto-provisioned)
                                              Skillset (auto-provisioned)
```

- **Agent** — the AI model with instructions and tools (Foundry)
- **Knowledge Base** — defines retrieval rules and groups knowledge sources (Search)
- **Knowledge Source** — connects a data source to a knowledge base (Search)
- **Index + managed resources** — auto-provisioned by Azure when you create a knowledge source

hoist manages all of these as local files, so you can version, review, and deploy them together.

## 1. Install hoist

```bash
brew install mklab-se/tap/hoist
```

Or see [INSTALL.md](INSTALL.md) for other methods. Make sure you're logged in to Azure:

```bash
az login
```

## 2. Initialize Your Project

```bash
mkdir my-rag-system && cd my-rag-system
hoist init . --template agentic
```

The `agentic` template sets up directories for all resource types, including preview agentic retrieval resources. hoist will discover your Azure services and create a `hoist.yaml` config.

> **Shortcut:** If you want to scaffold everything at once, skip steps 3-5 and run:
> ```bash
> hoist new agentic-rag my-system --model gpt-4o --container documents
> ```
> This creates a pre-wired agent, knowledge base, and knowledge source in one command. Jump to [step 6](#6-deploy-to-azure).

## 3. Create the Knowledge Base

```bash
hoist new knowledge-base regulatory-kb
```

Edit the generated file to add retrieval instructions:

```json
{
  "name": "regulatory-kb",
  "description": "Regulatory and compliance documents",
  "retrievalInstructions": "Find relevant regulatory passages. Prioritize exact legal text over summaries.",
  "outputMode": "extractiveData"
}
```

## 4. Create a Knowledge Source

```bash
hoist new knowledge-source regulatory --index regulatory-index --knowledge-base regulatory-kb
```

This creates the knowledge source definition. After pushing, Azure will auto-provision managed sub-resources (index, indexer, data source, skillset) — you don't need to create them manually.

Edit the generated file to configure your data connection:

```json
{
  "name": "regulatory",
  "indexName": "regulatory-index",
  "knowledgeBaseName": "regulatory-kb",
  "kind": "azureBlob",
  "description": "Regulatory PDFs and documents",
  "azureBlobParameters": {
    "containerName": "documents"
  }
}
```

## 5. Create the Agent

```bash
hoist new agent research-assistant --model gpt-4o
```

Edit the generated YAML to add instructions and connect to the knowledge base:

```yaml
kind: prompt
model: gpt-4o
instructions: |
  You are a research assistant specialized in regulatory compliance.
  Use the regulatory-kb knowledge base to find and cite relevant legal passages.
  Always provide specific section references when answering questions.
tools:
  - type: mcp
    server_label: regulatory-kb
    server_url: https://<your-search-service>.search.windows.net/knowledgebases/regulatory-kb/mcp
```

Replace `<your-search-service>` with your actual Azure AI Search service name (visible in `hoist.yaml`).

## 6. Deploy to Azure

Preview what will be pushed, then deploy:

```bash
# Preview changes
hoist push --all

# Deploy (after confirming the preview)
hoist push --all --force
```

## 7. Pull Back Managed Resources

After pushing the knowledge source, Azure auto-provisions managed sub-resources. Pull them back to have the complete picture locally:

```bash
hoist pull --all
```

Your project now contains all the auto-provisioned resources:

```
search/
  agentic-retrieval/
    knowledge-bases/
      regulatory-kb.json
    knowledge-sources/
      regulatory/
        regulatory.json              # Your knowledge source
        regulatory-index.json        # Auto-provisioned index
        regulatory-indexer.json      # Auto-provisioned indexer
        regulatory-datasource.json   # Auto-provisioned data source
        regulatory-skillset.json     # Auto-provisioned skillset
foundry/
  agents/
    research-assistant.yaml          # Your agent
```

## 8. Verify

```bash
# Check project status
hoist status

# See the full dependency graph
hoist describe
```

`hoist describe` shows how everything connects — from the agent through the knowledge base to the index.

## Next Steps

- **Version control**: `git init && git add -A && git commit -m "Initial RAG configuration"`
- **Diff against Azure**: `hoist diff --all` shows what changed since last sync
- **Add environments**: Configure `test` and `prod` environments in `hoist.yaml` for environment promotion
- **Connect your AI tool**: `hoist mcp install claude-code` lets Claude Code see and manage your RAG stack
- **CI/CD**: Use `hoist validate` in PR checks and `hoist push --all --force` on merge to main

See [README.md](README.md) for the full feature reference.

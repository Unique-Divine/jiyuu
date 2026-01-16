---
name: postgresql-psql
description: >-
  Comprehensive guide for PostgreSQL psql - the interactive terminal client for
  PostgreSQL. Use when connecting to PostgreSQL databases, executing queries,
  managing databases/tables, configuring connection options, formatting output,
  writing scripts, managing transactions, and using advanced psql features for
  database administration and development.
license: PostgreSQL
version: 1.0.0
---

# PostgreSQL psql Skill

PostgreSQL psql (PostgreSQL interactive terminal) is the primary command-line client for interacting with PostgreSQL databases. It provides both interactive query execution and powerful scripting capabilities for database management and administration.

## When to Use This Skill

Use this skill when:

- Connecting to PostgreSQL databases from the command line
- Executing SQL queries interactively
- Writing SQL scripts for automation
- Creating and managing databases and schemas
- Managing database objects (tables, views, indexes, functions)
- Backing up and restoring databases
- Configuring connections and authentication
- Formatting and exporting query results
- Managing transactions and permissions
- Debugging SQL queries
- Automating database administration tasks
- Setting up replication and high availability
- Creating stored procedures and functions

## Core Concepts

### REPL Model

- psql operates as an interactive REPL (Read-Eval-Print Loop)
- Accepts SQL commands and meta-commands (backslash commands)
- Maintains connection state across commands within a session
- Supports command history and editing

### Command Types

- **SQL Commands**: Standard SQL statements (SELECT, INSERT, UPDATE, DELETE, etc.)
- **Meta-Commands**: psql-specific commands prefixed with backslash (e.g., `\dt`, `\d`)
- **Backslash Commands**: Control query output, session variables, and psql behavior

### Connection Model

- Single database connection per session
- Can switch databases without reconnecting
- Connection state includes current database, user, and search path
- Environmental variables and .pgpass for credential management

## Reference Pointers

Detailed command lists, flags, and configuration options are available in [reference.md](./reference.md):

- Connect and Authenticate
- Meta-Commands
- Output and Formatting
- Files, Editor, and Scripting
- Command-Line Flags
- Import and Export
- Backup and Restore
- Performance and Debug Toolbox
- Troubleshooting

## Golden Paths

### 1. Connect using flags
```bash
psql -h localhost -p 5432 -U postgres -d mydb
```

### 2. Connect using URI
```bash
psql postgresql://user:pass@host:5432/dbname?sslmode=require
```

### 3. Inspect a table
```bash
\dt             # List all tables
\d table_name   # Describe specific table
```

### 4. Export query results to CSV
```bash
\copy (SELECT * FROM users) TO 'users.csv' WITH (FORMAT CSV, HEADER)
```

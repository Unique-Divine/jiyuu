# PostgreSQL psql Reference

## Connect and Authenticate

### Basic Connection Command

```bash
psql [OPTIONS] [DBNAME [USERNAME]]
```

### Common Connection Options

```bash
# Connect with username and host
psql -U username -h hostname -p 5432 -d database_name

# Connect using connection string
psql postgresql://username:password@hostname:5432/database_name

# Connect with password prompt
psql -U postgres -h localhost -W

# Connect to specific database on local machine
psql -d myapp_development

# Environment variables (alternative)
export PGUSER=postgres
export PGPASSWORD=mypassword
export PGHOST=localhost
export PGPORT=5432
export PGDATABASE=mydb
psql
```

### Connection String Formats

**Standard URI format**:

```
postgresql://[user[:password]@][netloc][:port][/dbname][?param1=value1&...]
```

**Example**:

```
postgresql://app_user:secretpass@db.example.com:5432/production_db?sslmode=require
```

### Authentication Methods

**Password file (.pgpass)**:

```
# ~/.pgpass (chmod 600)
hostname:port:database:username:password
localhost:5432:mydb:postgres:mypassword
*.example.com:5432:*:appuser:apppass
```

**Connection via SSH tunnel**:

```bash
ssh -L 5432:localhost:5432 user@remote-host
psql -U postgres -h localhost
```

### SSL/TLS Connection Options

```bash
# Require SSL
psql -h hostname -sslmode require -U username database

# Verify certificate
psql -h hostname -sslmode verify-full \
  -sslcert=/path/to/client-cert.crt \
  -sslkey=/path/to/client-key.key \
  -sslrootcert=/path/to/ca-cert.crt database

# SSL modes: disable, allow, prefer (default), require, verify-ca, verify-full
```

## Meta-Commands

### Database and Schema Navigation

```
\l or \list                    # List all databases
\l+ or \list+                  # List databases with sizes
\c or \connect DATABASE USER   # Connect to different database
\dn or \dn+                    # List schemas (namespaces)
\dt or \dt+                    # List tables in current schema
\di or \di+                    # List indexes
\dv or \dv+                    # List views
\dm or \dm+                    # List materialized views
\ds or \ds+                    # List sequences
\df or \df+                    # List functions/procedures
\da or \da+                    # List aggregates
\dT or \dT+                    # List data types
\dF or \dF+                    # List text search configurations
```

### Object Inspection Commands

```
\d or \d NAME                  # Describe table, view, index, sequence, or function
\d+ or \d+ NAME                # Extended description with details
\da PATTERN                    # List aggregate functions matching pattern
\db or \db+                    # List tablespaces
\dc or \dc+                    # List character set encodings
\dC or \dC+                    # List type casts
\dd or \dd+                    # List object descriptions/comments
\dD or \dD+                    # List domains
\de or \de+                    # List foreign data wrappers
\dE or \dE+                    # List foreign servers
\dF or \dF+                    # List text search configurations
\dFd or \dFd+                  # List text search dictionaries
\dFp or \dFp+                  # List text search parsers
\dFt or \dFt+                  # List text search templates
\dg or \dg+                    # List database roles/users
\dl or \dl+                    # List large objects (same as \lo_list)
\dL or \dL+                    # List procedural languages
\dO or \dO+                    # List collations
\dp or \dp+                    # List table access privileges
\dRp or \dRp+                  # List replication origins
\dRs or \dRs+                  # List replication subscriptions
\ds or \ds+                    # List sequences
\dt or \dt+                    # List tables
\dU or \dU+                    # List user mapping
\du or \du+                    # List roles
\dv or \dv+                    # List views
\dx or \dx+                    # List extensions
\dX or \dX+                    # List extended statistics
```

### Formatting and Output Commands

```
\a                             # Toggle between aligned and unaligned output
\C [STRING]                    # Set table title
\f [STRING]                    # Set field separator for unaligned output
\H                             # Toggle HTML output mode
\pset OPTION [VALUE]           # Set output option (detailed below)
\t [on|off]                    # Toggle tuple-only output (no headers/footers)
\T [STRING]                    # Set HTML table tag attributes
\x or \x [on|off|auto]         # Toggle expanded/vertical output
\g or \g [FILENAME|COMMAND]    # Execute query and send output to file/command
```

### File and History Commands

```
\copy QUERY TO FILENAME [FORMAT]          # Client-side COPY (requires fewer permissions)
\copy QUERY TO STDOUT                     # Copy to standard output
\copy TABLE FROM FILENAME [FORMAT]        # Import data from file
\e or \edit                               # Edit current query buffer in editor
\e FILENAME                               # Edit file in editor
\ef [FUNCNAME]                            # Edit function definition
\ev [VIEWNAME]                            # Edit view definition
\w FILENAME or \write FILENAME            # Write current query buffer to file
\i FILENAME or \include FILENAME          # Execute SQL commands from file
\ir FILENAME or \include_relative FILE    # Execute relative path file
\s [FILENAME]                             # Show command history (or save to file)
\o FILENAME or \out FILENAME              # Send all output to file
\o                                        # Return output to terminal
```

### Batch and Script Commands

```
\echo TEXT                     # Print text (useful in scripts)
\errverbose                    # Show last error in verbose form
\q or \quit                    # Quit psql
\! COMMAND or \shell COMMAND   # Execute shell command
\cd DIRECTORY                  # Change working directory
\pwd                           # Print current working directory
\set VARIABLE VALUE            # Set psql variable
\unset VARIABLE                # Unset psql variable
\setenv VARNAME VALUE          # Set environment variable
\getenv VARNAME                # Get environment variable value
\prompt [TEXT] VARIABLE        # Prompt user for input and set variable
```

### Transaction Commands

```
\begin or BEGIN                # Start transaction
\commit or COMMIT              # Commit transaction
\rollback or ROLLBACK          # Rollback transaction
\savepoint NAME                # Create savepoint
\release SAVEPOINT             # Release savepoint
\rollback TO SAVEPOINT         # Rollback to savepoint
```

### Information Commands

```
\d+ TABLENAME                  # Show table with extended info and storage info
\dt *.*                        # List all tables in all schemas
\dn *                          # List all schemas
\du                            # List all users/roles
\db                            # List tablespaces
\dx                            # List installed extensions
\h or \help                    # List available SQL commands
\h COMMAND or \help COMMAND    # Show help for specific SQL command
\?                             # Show psql help
\copyright                     # Show PostgreSQL copyright/license info
\version or SELECT version()   # Show PostgreSQL version
```

## Output and Formatting

### \pset Options

(See examples in configuration below for common settings like border, linestyle, and expanded mode.)

### Output Formats Comparison

```
-- Aligned (default)
\pset format aligned

-- CSV
\pset format csv
\copy (SELECT * FROM users) TO STDOUT WITH (FORMAT CSV);

-- HTML
\pset format html
SELECT * FROM users LIMIT 5;

-- LaTeX
\pset format latex
SELECT * FROM users LIMIT 5;

-- Expanded (vertical)
\x
SELECT * FROM users LIMIT 1;
```

## Files, Editor, and Scripting

### Scripting with psql

#### Running SQL Files

```bash
# Execute file
psql -d mydb -f script.sql

# Execute with output to file
psql -d mydb -f script.sql -o results.txt

# Execute with error stopping
psql -d mydb -f script.sql --on-error-stop

# Execute in single transaction
psql -d mydb -f script.sql -s

# Multiple files (executed in order)
psql -d mydb -f init.sql -f seed.sql -f verify.sql
```

#### SQL Script Best Practices

```sql
-- sample_script.sql

-- Set execution mode
\set ON_ERROR_STOP ON
\set QUIET OFF

-- Drop existing objects if needed
DROP TABLE IF EXISTS temp_table;

-- Create table
CREATE TABLE temp_table (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL
);

-- Insert data
INSERT INTO temp_table (name) VALUES
  ('Record 1'),
  ('Record 2'),
  ('Record 3');

-- Verify results
SELECT * FROM temp_table;

-- Cleanup
DROP TABLE temp_table;

-- Report
\echo 'Script completed successfully!'
```

#### Dynamic SQL Scripts

```bash
#!/bin/bash

# Bash script with psql variables
DATABASE="myapp_db"
TABLE_NAME="users"
SCHEMA_NAME="public"

# Execute with variable substitution
psql -d $DATABASE -v table_name=$TABLE_NAME \
  -v schema_name=$SCHEMA_NAME -c "
  SELECT COUNT(*) FROM :schema_name.:table_name;
"

# Loop through databases
for db in $(psql -l | awk '{print $1}'); do
  if [[ ! "$db" =~ "template" ]]; then
    echo "Backing up $db..."
    pg_dump $db > /backups/$db.sql
  fi
done
```

### Useful .psqlrc Shortcuts

```bash
# Add to ~/.psqlrc for convenient shortcuts
\set dbsize 'SELECT pg_size_pretty(pg_database_size(current_database()))'
\set uptime 'SELECT now() - pg_postmaster_start_time() AS uptime'
\set psql_version 'SELECT version()'
\set table_sizes 'SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'\''.\'\'||tablename)) FROM pg_tables ORDER BY pg_total_relation_size(schemaname||'\''.\'\'||tablename) DESC'

# Usage in psql:
# :dbsize
# :table_sizes
```

### Configuration File (~/.psqlrc)

```bash
# Auto-load on psql startup
# Set default options
\set QUIET ON
\set SQLHISTSIZE 10000

# Configure output
\pset null '[NULL]'
\pset border 2
\pset linestyle unicode
\pset expanded auto
\pset pager always

# Define useful variables
\set conn_user 'SELECT current_user;'
\set dbsize 'SELECT pg_size_pretty(pg_database_size(current_database()));'
\set tables 'SELECT tablename FROM pg_tables WHERE schemaname = ''public'';'
\set functions 'SELECT proname FROM pg_proc;'

# Define shortcuts
\set uptime 'SELECT now() - pg_postmaster_start_time() AS uptime;'
\set locks 'SELECT pid, usename, pg_blocking_pids(pid) as blocked_by, query, state FROM pg_stat_activity WHERE cardinality(pg_blocking_pids(pid)) > 0;'

# Set timing
\timing ON

# Connect to default database
\c mydb
```

### Variable Substitution

```sql
-- Using :variable syntax
\set table_name mytable
SELECT * FROM :table_name;

-- Using :'variable' for literal strings
\set schema_name public
SELECT * FROM :"schema_name".mytable;

-- Using :'variable' syntax in string context
\set username 'postgres'
SELECT * FROM pg_tables WHERE tableowner = :'username';

-- Using :' ' for identifier quoting
\set id_name "customTable"
SELECT * FROM :"id_name";
```

### Prompt Customization

```bash
# Set custom prompts
psql -v PROMPT1='user@db> '
psql -v PROMPT1='%/%R%# '   # database/role#

# In .psqlrc
\set PROMPT1 '%n@%m:%>/%/ %R%# '
\set PROMPT2 '> '
\set PROMPT3 '>> '
```

### Built-in Variables

```bash
# Prompt variables
psql -v PROMPT1='%/%R%# '     # Set primary prompt
psql -v PROMPT2='%R%# '       # Set continuation prompt
psql -v PROMPT3='>> '         # Set output mode prompt

# Prompt expansion codes:
# %n = Database user name
# %m = Database server hostname (first part)
# %> = Database server hostname full
# %p = Database server port
# %d = Database name
# %/ = Current schema
# %~ = Like %/ but ~  if schema matches user name
# %# = # if superuser, > otherwise
# %? = Last query result status
# %% = Literal %
# %[..%] = Invisible characters (for terminal control sequences)
```

## Command-Line Flags

### Connection Options

```bash
-h, --host=HOSTNAME           # Server host name (default: localhost)
-p, --port=PORT               # Server port (default: 5432)
-U, --username=USERNAME       # PostgreSQL user name (default: $USER)
-d, --dbname=DBNAME           # Database name to connect
-w, --no-password             # Never prompt for password
-W, --password                # Force password prompt
```

### Output and Formatting Options

```bash
-A, --no-align                # Unaligned table output mode
-c, --command=COMMAND         # Run single command and exit
-C, --copy-only               # (deprecated, use \copy instead)
-d, --dbname=DBNAME           # Specify database
-E, --echo-hidden             # Display internal queries
-e, --echo-all                # Display each command before sending
-b, --echo-errors             # Display failed commands
-f, --file=FILENAME           # Execute commands from file
-F, --field-separator=CHAR    # Set field separator for unaligned output
-H, --html                    # HTML table output mode
-l, --list                    # List available databases and exit
-L, --log-file=FILENAME       # Log session to file
-n, --no-readline             # Disable readline (line editing)
-o, --output=FILENAME         # Write results to file
-P, --pset=VARIABLE=VALUE     # Set printing option
-q, --quiet                   # Run quietly (no banner, single-line mode)
-R, --record-separator=CHAR   # Set record separator for unaligned output
-S, --single-step             # Single-step mode (confirm each command)
-s, --single-transaction      # Execute file in single transaction
-t, --tuples-only             # Print rows only (no headers/footers)
-T, --table-attr=STRING       # Set HTML table tag attributes
-v, --set=VARIABLE=VALUE      # Set psql variable
-V, --version                 # Show version and exit
-x, --expanded                # Expanded table output mode
-X, --no-psqlrc               # Do not read ~/.psqlrc startup file
-1, --single-line             # End of line terminates SQL command
```

### Other Options

```bash
-a, --all                     # (deprecated)
-j, --job=NUM                 # (for parallel dumps with pg_dump)
--help                        # Show help message
--version                     # Show version
--on-error-stop               # Stop on first error
```

## Import and Export

### COPY Commands

```sql
-- Server-side COPY (requires superuser for file operations)
COPY users (id, name, email)
TO '/tmp/users.csv'
WITH (FORMAT CSV, HEADER TRUE, QUOTE '"', ESCAPE '\\');

-- Import CSV
COPY users (id, name, email)
FROM '/tmp/users.csv'
WITH (FORMAT CSV, HEADER TRUE, QUOTE '"', ESCAPE '\\');

-- Tab-separated values
COPY users TO '/tmp/users.tsv' WITH (FORMAT TEXT, DELIMITER E'\t');

-- With NULL handling
COPY users TO '/tmp/users.csv'
WITH (FORMAT CSV, NULL 'N/A', QUOTE '"');
```

### Client-side COPY (\copy)

```bash
# Export to CSV (from psql)
\copy users TO '/home/user/users.csv' WITH (FORMAT CSV, HEADER)

# Export with query results
\copy (SELECT id, name, email FROM users WHERE active = true) \
  TO '/tmp/active_users.csv' WITH (FORMAT CSV, HEADER)

# Import CSV
\copy users (id, name, email) FROM '/tmp/users.csv' WITH (FORMAT CSV, HEADER)

# Export to stdout (pipe to file)
\copy users TO STDOUT WITH (FORMAT CSV, HEADER) > users.csv

# Import from stdin
cat users.csv | \copy users FROM STDIN WITH (FORMAT CSV, HEADER)
```

## Backup and Restore

### Using pg_dump and pg_restore

```bash
# Dump entire database
pg_dump -d mydb -U postgres > mydb_backup.sql

# Dump with custom format (compressed)
pg_dump -d mydb -Fc > mydb_backup.dump

# Dump specific table
pg_dump -d mydb -t users > users_backup.sql

# Dump with data only
pg_dump -d mydb -a > mydb_data.sql

# Dump schema only
pg_dump -d mydb -s > mydb_schema.sql

# Restore from SQL file
psql -d mydb_restored -f mydb_backup.sql

# Restore from custom format
pg_restore -d mydb_restored mydb_backup.dump

# List contents of dump
pg_restore -l mydb_backup.dump
```

### Backup and Recovery

#### Database Backup Strategies

```bash
# Full database backup (custom format)
pg_dump -d production_db -Fc -j 4 > backup.dump

# Backup with compression
pg_dump -d production_db -Fc -Z 9 > backup.dump

# Parallel backup (faster for large databases)
pg_dump -d production_db -Fd -j 4 -f backup_dir

# Backup specific schemas
pg_dump -d production_db -n public -n app > schemas.sql

# Backup with custom format (allows selective restore)
pg_dump -d production_db -Fc > backup.dump

# View backup contents
pg_restore -l backup.dump | less

# Restore specific table
pg_restore -d restored_db -t users backup.dump

# List available backups
pg_dump -U postgres -l -w postgres
```

#### Point-in-Time Recovery

```bash
# Full backup
pg_dump -d mydb > base_backup.sql

# Enable WAL archiving (in postgresql.conf)
wal_level = replica
archive_mode = on
archive_command = 'cp %p /archive/%f'

# Restore to point in time
pg_restore -d recovered_db base_backup.sql
# Then apply WAL files up to target time
```

## Performance and Debug Toolbox

### Performance and Debugging

#### Query Analysis

```sql
-- Show query execution plan
EXPLAIN SELECT * FROM users WHERE id = 1;

-- Detailed analysis with actual execution
EXPLAIN ANALYZE SELECT * FROM users WHERE id = 1;

-- Show more details
EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
  SELECT * FROM users WHERE active = true;

-- JSON output for programmatic parsing
EXPLAIN (FORMAT JSON, ANALYZE)
  SELECT COUNT(*) FROM users;
```

#### Viewing Query Performance

```sql
-- Current queries
SELECT pid, usename, state, query FROM pg_stat_activity;

-- Long-running queries
SELECT pid, usename, now() - query_start AS duration, query
FROM pg_stat_activity
WHERE state != 'idle'
ORDER BY query_start;

-- Blocking queries
SELECT blocked_pid, blocking_pid, blocked_statement, blocking_statement
FROM pg_stat_statements;

-- Table sizes
SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) AS size
FROM pg_tables
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;

-- Database size
SELECT pg_size_pretty(pg_database_size('mydb'));
```

#### Setting Timing

```bash
# Enable query timing
\timing ON

# Disable query timing
\timing OFF

# In batch mode
psql -d mydb -c "\timing ON" -f script.sql
```

#### Query Logging

```bash
# Log all queries to file
psql -d mydb -L query.log -f script.sql

# Show internal queries (system queries)
psql -d mydb -E
```

## Troubleshooting

### Connection Issues

```bash
# Verbose connection diagnostics
psql -d mydb -v verbose=on --echo-queries

# Check connection settings
psql --version
psql -d postgres -c "SHOW password_encryption;"

# TCP/IP connectivity test
psql -h hostname -d postgres -U postgres -c "SELECT 1;"
```

### Common Error Messages

```
FATAL: password authentication failed
  → Check password, user exists, .pgpass has correct permissions (600)

FATAL: no pg_hba.conf entry for host
  → Database server's pg_hba.conf needs connection rule

FATAL: database "name" does not exist
  → Create database or check database name spelling

ERROR: permission denied for schema
  → Grant USAGE on schema to user

ERROR: syntax error
  → Check SQL syntax, use \h for help on commands
```

### Performance Issues

```sql
-- Find slow queries
SELECT * FROM pg_stat_statements
ORDER BY total_time DESC
LIMIT 10;

-- Check for missing indexes
SELECT schemaname, tablename, attname
FROM pg_stat_user_tables, pg_attribute
WHERE pg_stat_user_tables.relid = pg_attribute.attrelid
AND seq_scan > 0;

-- Check cache efficiency
SELECT
  sum(heap_blks_read) as heap_read,
  sum(heap_blks_hit)  as heap_hit,
  sum(heap_blks_hit) / (sum(heap_blks_hit) + sum(heap_blks_read)) AS ratio
FROM pg_statio_user_tables;
```

## Resources

- Official PostgreSQL Documentation: https://www.postgresql.org/docs/
- psql Manual: https://www.postgresql.org/docs/current/app-psql.html
- PostgreSQL Wiki: https://wiki.postgresql.org/
- pgAdmin (GUI tool): https://www.pgadmin.org/
- DBA Best Practices: https://www.postgresql.org/docs/current/sql-syntax.html

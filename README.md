# Send Directory

```bash
# Receiver process
sync-directory --port 8080 --output-dir ~/Downloads/output_dir

# Sender process
sync-directory --from ~/Downloads/input_dir --to ws://localhost:8080
```

Once the sender command is completed, the receiver process should exit automatically, and the contents of `input_dir` will be found under `output_dir`.
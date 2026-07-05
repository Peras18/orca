with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''    pub fn spawn_block_poller(self: &Arc<Self>) {
        let engine = self.clone();'''

new = '''    pub fn spawn_block_poller(&self) {
        let engine = self.clone();'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")

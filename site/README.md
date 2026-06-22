# rover-fetch.com

Docusaurus site for Rover. Static, agent-first, deployed to Netlify.

## Develop

```sh
nvm use            # Node 26.3.1 (.nvmrc)
sfw pnpm install   # ALWAYS through Socket Firewall
sfw pnpm start     # local dev server
sfw pnpm build     # static output -> build/
```

## Rules
- Every install/add/update goes through `sfw pnpm` — never bare pnpm.
- No package version younger than 3 days (`minimumReleaseAge` in pnpm-workspace.yaml).
- No dependency build scripts run unless added to `allowBuilds` in pnpm-workspace.yaml.

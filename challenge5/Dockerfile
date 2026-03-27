FROM ghcr.io/openclaw/openclaw:main-slim

USER node
WORKDIR /home/node

RUN mkdir -p /home/node/.openclaw/workspace/skills
COPY --chown=node:node ./skills /home/node/.openclaw/workspace/skills

WORKDIR /home/node/.openclaw/workspace/skills/multiversx/moltbot-starter-kit
# Set NODE_ENV to development to ensure devDependencies like typescript are installed
ENV NODE_ENV=development
RUN npm install
RUN npm run build

WORKDIR /home/node/.openclaw/workspace/skills/spam-skill/scripts
RUN npm install

# Revert to production if needed
ENV NODE_ENV=production

WORKDIR /app
CMD ["node", "openclaw.mjs", "gateway", "--allow-unconfigured"]


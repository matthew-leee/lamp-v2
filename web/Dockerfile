# Multi-stage production Dockerfile for Next.js storefront & admin studio (OPS-01).
FROM node:22-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm install
COPY . .
RUN mkdir -p public/uploads public/demo-assets
ARG NEXT_PUBLIC_BRAND_NAME
ARG NEXT_PUBLIC_BRAND_NAME_NATIVE
ARG NEXT_PUBLIC_SITE_URL
ARG NEXT_PUBLIC_API_URL
ARG NEXT_PUBLIC_CURRENCY=HKD
ARG NEXT_PUBLIC_LOCALES=en,zh-HK
ENV NEXT_PUBLIC_BRAND_NAME=${NEXT_PUBLIC_BRAND_NAME}
ENV NEXT_PUBLIC_BRAND_NAME_NATIVE=${NEXT_PUBLIC_BRAND_NAME_NATIVE}
ENV NEXT_PUBLIC_SITE_URL=${NEXT_PUBLIC_SITE_URL}
ENV NEXT_PUBLIC_API_URL=${NEXT_PUBLIC_API_URL}
ENV NEXT_PUBLIC_CURRENCY=${NEXT_PUBLIC_CURRENCY}
ENV NEXT_PUBLIC_LOCALES=${NEXT_PUBLIC_LOCALES}
RUN npm run build

FROM node:22-alpine AS runtime
WORKDIR /app
ENV NODE_ENV=production
COPY --from=builder --chown=node:node /app/package.json ./
COPY --from=builder --chown=node:node /app/node_modules ./node_modules
COPY --from=builder --chown=node:node /app/.next ./.next
COPY --from=builder --chown=node:node /app/public ./public
USER node
EXPOSE 3002
CMD ["npm", "run", "start"]

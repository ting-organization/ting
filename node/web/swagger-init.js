/* All assets and API requests stay on this node; no online validator or CDN. */
window.addEventListener('DOMContentLoaded', () => {
  window.ui = SwaggerUIBundle({
    url: '/openapi.json',
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis],
    layout: 'BaseLayout',
    deepLinking: true,
    filter: true,
    displayRequestDuration: true,
    defaultModelsExpandDepth: 1,
    docExpansion: 'list',
    validatorUrl: null,
    supportedSubmitMethods: ['get', 'post'],
    persistAuthorization: false,
    queryConfigEnabled: false
  });
});

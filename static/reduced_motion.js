// @license http://www.gnu.org/licenses/agpl-3.0.html AGPL-3.0
// Autoplay is motion the visitor did not ask for. When their system asks for less motion,
// stop the videos the page started on its own; the controls still play them.
if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    document.querySelectorAll('video[autoplay]').forEach(function (video) {
        video.removeAttribute('autoplay');
        video.pause();
    });
}
// @license-end

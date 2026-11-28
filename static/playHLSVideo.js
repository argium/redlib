// @license http://www.gnu.org/licenses/agpl-3.0.html AGPL-3.0
(function () {
    // Fatal errors are recovered this many times per kind before the player gives up.
    const MAX_RECOVERIES = 3;
    const qualitySetting = document.body.dataset.videoQuality;
    // Autoplay is motion the visitor did not ask for when their system asks for less of it.
    const prefersReducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (Hls.isSupported()) {
        var videoSources = document.querySelectorAll("video source[type='application/vnd.apple.mpegurl']");
        videoSources.forEach(function (source) {
            var playlist = source.src;

            var oldVideo = source.parentNode;
            var autoplay = oldVideo.classList.contains("hls_autoplay") && !prefersReducedMotion;

            // If HLS is supported natively then don't use hls.js
            if (oldVideo.canPlayType(source.type) === "probably") {
                if (autoplay) {
                    oldVideo.play();
                }
                return;
            }

            // The copy below has its sources removed, so remember the MP4 to fall back to
            var mp4Source = oldVideo.querySelector("source[type='video/mp4']");
            var mp4Url = mp4Source ? mp4Source.src : "";

            // Replace video with copy that will have all "source" elements removed
            var newVideo = oldVideo.cloneNode(true);
            var allSources = newVideo.querySelectorAll("source");
            allSources.forEach(function (source) {
                source.remove();
            });

            // Empty source to enable play event
            newVideo.src = "about:blank";

            oldVideo.parentNode.replaceChild(newVideo, oldVideo);

            function getIndexOfDefault(length) {
                switch (qualitySetting) {
                    case 'best':
                        return length - 1;
                    case 'medium':
                        return Math.floor(length / 2);
                    case 'worst':
                        return 0;
                    default:
                        return length - 1;
                }
            }

            function initializeHls() {
                newVideo.removeEventListener('play', initializeHls);
                var hls = new Hls({ autoStartLoad: false });
                var recoveries = { network: 0, media: 0 };
                var qualitySelector = null;
                var gaveUp = false;

                function giveUp() {
                    gaveUp = true;
                    hls.destroy();
                    if (qualitySelector) {
                        qualitySelector.remove();
                    }

                    var message = document.createElement('p');
                    message.classList.add('hls_error');
                    message.setAttribute('role', 'status');
                    newVideo.parentNode.appendChild(message);
                    if (mp4Url) {
                        message.textContent = 'This video could not be loaded with audio. Playing it without audio instead.';
                        newVideo.src = mp4Url;
                        newVideo.load();
                        var played = newVideo.play();
                        if (played) {
                            // Autoplay can be blocked; the visitor can still press play.
                            played.catch(function () {});
                        }
                    } else {
                        message.textContent = 'This video could not be played.';
                    }
                }

                hls.loadSource(playlist);
                hls.attachMedia(newVideo);
                hls.on(Hls.Events.MANIFEST_PARSED, function () {
                    hls.loadLevel = getIndexOfDefault(hls.levels.length);
                    var availableLevels = hls.levels.map(function(level) {
                        return {
                            height: level.height,
                            width: level.width,
                            bitrate: level.bitrate,
                        };
                    });

                    qualitySelector = addQualitySelector(newVideo, hls, availableLevels);

                    hls.startLoad();
                    newVideo.play();
                });

                // Network errors reset once a fragment downloads; media errors reset only once
                // playback actually advances, so a downloaded but unplayable fragment cannot
                // renew the media budget forever.
                hls.on(Hls.Events.FRAG_LOADED, function () {
                    recoveries.network = 0;
                });
                var lastPlaybackTime = 0;
                newVideo.addEventListener('timeupdate', function () {
                    if (newVideo.currentTime > lastPlaybackTime) {
                        lastPlaybackTime = newVideo.currentTime;
                        recoveries.media = 0;
                    }
                });

                hls.on(Hls.Events.ERROR, function (event, data) {
                    console.error("HLS error", data);
                    if (!data.fatal || gaveUp) {
                        return;
                    }

                    if (data.type === Hls.ErrorTypes.NETWORK_ERROR && recoveries.network < MAX_RECOVERIES) {
                        // Wait 1 s, 2 s, then 4 s so a dead stream is not hammered.
                        setTimeout(function () {
                            if (!gaveUp) {
                                hls.startLoad();
                            }
                        }, 1000 * 2 ** recoveries.network++);
                    } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR && recoveries.media < MAX_RECOVERIES) {
                        recoveries.media++;
                        hls.recoverMediaError();
                    } else {
                        giveUp();
                    }
                });
            }

            function addQualitySelector(videoElement, hlsInstance, availableLevels) {
                var qualitySelector = document.createElement('select');
                qualitySelector.classList.add('quality-selector');
                qualitySelector.setAttribute('aria-label', 'Video quality');
                var defaultIndex = getIndexOfDefault(availableLevels.length);
                availableLevels.forEach(function (level, index) {
                    var option = document.createElement('option');
                    option.value = index.toString();
                    var bitrate = (level.bitrate / 1_000).toFixed(0);
                    option.text = level.height + 'p (' + bitrate + ' kbps)';
                    if (index === defaultIndex) {
                        option.selected = "selected";
                    }
                    qualitySelector.appendChild(option);
                });
                qualitySelector.selectedIndex = defaultIndex;
                qualitySelector.addEventListener('change', function () {
                    var selectedIndex = qualitySelector.selectedIndex;
                    hlsInstance.nextLevel = selectedIndex;
                    hlsInstance.startLoad();
                });

                videoElement.parentNode.appendChild(qualitySelector);
                return qualitySelector;
            }

            newVideo.addEventListener('play', initializeHls);

            if (autoplay) {
                newVideo.play();
            }
        });
    } else {
        var videos = prefersReducedMotion ? [] : document.querySelectorAll("video.hls_autoplay");
        videos.forEach(function (video) {
            video.setAttribute("autoplay", "");
        });
    }
})();
// @license-end

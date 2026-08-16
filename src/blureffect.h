#pragma once

#include <QObject>
#include <QHash>
#include <QRectF>
#include <QWaylandClientExtension>

#include "qwayland-ext-background-effect-v1.h"

class QQuickWindow;

/**
 * Asks the compositor to blur whatever is behind the dock panel.
 *
 * This uses ext-background-effect-v1, the standard staging protocol. It replaces
 * org_kde_kwin_blur / org_kde_kwin_contrast, which KWin 6.7 no longer implements.
 * Unlike the plasma window-management protocol, this one is unprivileged, so no
 * .desktop entry is required for it.
 */
class BlurEffect : public QWaylandClientExtensionTemplate<BlurEffect>,
                   public QtWayland::ext_background_effect_manager_v1
{
    Q_OBJECT

public:
    BlurEffect();
    ~BlurEffect() override;

    /**
     * Blurs a capsule-shaped area of the window.
     *
     * wl_region only understands rectangles, so the rounded end caps are
     * approximated by a stack of horizontal slices. Without that the compositor
     * would blur the full bounding box and the corners outside the capsule
     * would show as unblurred squares.
     */
    void setCapsuleBlurRegion(QQuickWindow *window, const QRectF &rect, qreal radius);

    /// Removes any blur previously applied to this window.
    void clear(QQuickWindow *window);

private:
    /// Number of slices used per rounded cap. 12 keeps the stair-stepping below
    /// one pixel at the panel sizes we use, without bloating the region.
    static constexpr int s_capSlices = 12;

    struct ext_background_effect_surface_v1 *effectForWindow(QQuickWindow *window);

    QHash<QQuickWindow *, struct ext_background_effect_surface_v1 *> m_effects;
};

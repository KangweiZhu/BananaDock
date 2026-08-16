#pragma once

#include <QObject>
#include <QWaylandClientExtension>

#include "qwayland-kde-screen-edge-v1.h"

class QQuickWindow;

/**
 * Auto-hide, delegated to the compositor.
 *
 * KWin does the actual work here: once the edge is activated it hides the
 * surface, watches the screen border, and slides the dock back in when the
 * pointer arrives. We only decide when to arm and disarm it, so there is no
 * client-side hide animation or edge-detection polling to get wrong.
 *
 * Requires the surface to already have the layer_surface role, otherwise the
 * compositor raises an invalid_role protocol error.
 */
class ScreenEdge : public QWaylandClientExtensionTemplate<ScreenEdge>,
                   public QtWayland::kde_screen_edge_manager_v1
{
    Q_OBJECT

public:
    ScreenEdge();
    ~ScreenEdge() override;

    /// Binds the bottom screen edge to this window. Safe to call more than once.
    void attach(QQuickWindow *window);

    /// true hides the dock and arms the edge; false pins it visible.
    void setHidden(bool hidden);

    bool isAvailable() const;

private:
    struct kde_auto_hide_screen_edge_v1 *m_edge = nullptr;
    bool m_hidden = false;
};

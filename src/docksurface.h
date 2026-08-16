#pragma once

#include <QObject>
#include <QRect>

#include <memory>

class BlurEffect;

class QQuickWindow;

/**
 * Turns a QQuickWindow into a wlr-layer-shell dock surface.
 *
 * This is the only part of the project that has to be C++: anchoring, the
 * exclusive zone and the input region can only be set through the layer-shell
 * protocol, which QML has no access to.
 */
class DockSurface : public QObject
{
    Q_OBJECT

public:
    explicit DockSurface(QObject *parent = nullptr);
    ~DockSurface() override;

    /// Must be called before window->show(), otherwise Qt creates the surface
    /// with a plain xdg-shell role instead.
    void attach(QQuickWindow *window);

    /**
     * Height of the strut other windows must avoid. The surface itself is much
     * taller than this (magnified icons need room to grow upwards), but windows
     * only need to keep clear of the panel at its resting size -- which is
     * exactly what macOS does.
     */
    Q_INVOKABLE void setExclusiveZone(int zone);

    /**
     * Restricts which part of the surface accepts pointer events. The surface
     * spans the whole bottom edge of the screen; without this, the transparent
     * area around and above the dock would swallow clicks meant for the windows
     * underneath.
     */
    Q_INVOKABLE void setInputRegion(int x, int y, int width, int height);

    /**
     * Blurs the compositor's view of whatever sits behind the panel. Takes the
     * capsule radius so the blur follows the panel's rounded shape instead of
     * its bounding box.
     */
    Q_INVOKABLE void setBlurRegion(qreal x, qreal y, qreal width, qreal height, qreal radius);

private:
    QQuickWindow *m_window = nullptr;
    int m_exclusiveZone = 0;
    std::unique_ptr<BlurEffect> m_blur;
};

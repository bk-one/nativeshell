use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    rc::{Rc, Weak},
    time::Duration,
};

use cocoa::{
    appkit::{
        NSEvent, NSEventType, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    },
    base::{id, nil, BOOL, NO, YES},
    foundation::{NSArray, NSInteger, NSPoint, NSRect, NSSize, NSString, NSUInteger},
};
use cocoa::{
    appkit::{NSScreen, NSWindowTabbingMode},
    foundation::NSProcessInfo,
};

use objc::{
    declare::ClassDecl,
    rc::{StrongPtr, WeakPtr},
};
use objc::{
    rc::autoreleasepool,
    runtime::{Class, Object, Sel},
};

use NSEventType::{
    NSLeftMouseDown, NSLeftMouseUp, NSMouseEntered, NSMouseExited, NSMouseMoved, NSRightMouseDown,
    NSRightMouseUp,
};

use crate::{
    codec::Value,
    shell::{
        api_model::{
            DragEffect, DragRequest, PopupMenuRequest, PopupMenuResponse, WindowFrame,
            WindowGeometry, WindowGeometryFlags, WindowGeometryRequest, WindowStyle,
        },
        Context, PlatformWindowDelegate, Point, Size,
    },
    util::{LateRefCell, OkLog},
};

use super::{
    drag_context::{DragContext, NSDragOperation},
    engine::PlatformEngine,
    error::{PlatformError, PlatformResult},
    menu::PlatformMenu,
    utils::*,
};

pub type PlatformWindowType = StrongPtr;

pub struct PlatformWindow {
    context: Rc<Context>,
    platform_window: PlatformWindowType,
    parent_platform_window: Option<WeakPtr>,
    platform_delegate: StrongPtr,
    weak_self: LateRefCell<Weak<PlatformWindow>>,
    delegate: Weak<dyn PlatformWindowDelegate>,
    modal_close_callback: RefCell<Option<Box<dyn FnOnce(PlatformResult<Value>)>>>,
    ready_to_show: Cell<bool>,
    show_when_ready: Cell<bool>,
    drag_context: LateRefCell<DragContext>,
    last_event: RefCell<HashMap<u64, StrongPtr>>,
    ignore_enter_leave_until: Cell<f64>,
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    pub static NSPasteboardTypeFileURL: id;
}

impl PlatformWindow {
    pub fn new(
        context: Rc<Context>,
        delegate: Weak<dyn PlatformWindowDelegate>,
        parent: Option<Rc<PlatformWindow>>,
    ) -> Self {
        autoreleasepool(|| unsafe {
            let rect = NSRect::new(NSPoint::new(400.0, 400.0), NSSize::new(400.0, 400.0));
            let style = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask
                | NSWindowStyleMask::NSResizableWindowMask
                | NSWindowStyleMask::NSMiniaturizableWindowMask;
            let window: id = msg_send![WINDOW_CLASS.0, alloc];
            let window = window.initWithContentRect_styleMask_backing_defer_(
                rect,
                style,
                cocoa::appkit::NSBackingStoreType::NSBackingStoreBuffered,
                NO,
            );
            let window = StrongPtr::new(window);
            window.setReleasedWhenClosed_(NO);

            NSWindow::setAllowsAutomaticWindowTabbing_(*window, NO);
            NSWindow::setTabbingMode_(*window, NSWindowTabbingMode::NSWindowTabbingModeDisallowed);

            let platform_delegate: id = msg_send![WINDOW_DELEGATE_CLASS.0, new];
            let platform_delegate = StrongPtr::new(platform_delegate);

            window.setDelegate_(*platform_delegate);

            Self {
                context,
                platform_window: window,
                parent_platform_window: parent.map(|w| w.platform_window.weak()),
                platform_delegate,
                weak_self: LateRefCell::new(),
                delegate,
                modal_close_callback: RefCell::new(None),
                ready_to_show: Cell::new(false),
                show_when_ready: Cell::new(false),
                last_event: RefCell::new(HashMap::new()),
                drag_context: LateRefCell::new(),
                ignore_enter_leave_until: Cell::new(0.0),
            }
        })
    }

    pub fn assign_weak_self(&self, weak: Weak<PlatformWindow>, engine: &PlatformEngine) {
        self.weak_self.set(weak.clone());

        unsafe {
            let state_ptr = Box::into_raw(Box::new(weak.clone())) as *mut c_void;
            (**self.platform_delegate).set_ivar("imState", state_ptr);

            let state_ptr = Box::into_raw(Box::new(weak.clone())) as *mut c_void;
            (**self.platform_window).set_ivar("imState", state_ptr);

            let () =
                msg_send![*self.platform_window, setContentViewController: *engine.view_controller];

            // Temporarily set non empty window size so that flutter engine doesn't complain
            NSWindow::setContentSize_(*self.platform_window, Size::wh(1.0, 1.0).into());
        }

        let drag_context = DragContext::new(self.context.clone(), weak);
        drag_context.register(*self.platform_window);
        self.drag_context.set(drag_context);
    }

    pub fn get_platform_window(&self) -> PlatformWindowType {
        self.platform_window.clone()
    }

    pub fn set_geometry(
        &self,
        geometry: WindowGeometryRequest,
    ) -> PlatformResult<WindowGeometryFlags> {
        autoreleasepool(|| unsafe {
            let geometry = geometry.filtered_by_preference();

            let mut res = WindowGeometryFlags {
                ..Default::default()
            };

            // for modal window position is handled by the system
            let modal = self.is_modal();

            if !modal {
                if let Some(frame_origin) = geometry.frame_origin {
                    self.set_frame_origin(frame_origin);
                    res.frame_origin = true;
                }
            }

            if let Some(frame_size) = geometry.frame_size {
                self.set_frame_size(frame_size);
                res.frame_size = true;
            }

            if !modal {
                if let Some(content_origin) = geometry.content_origin {
                    self.set_content_position(content_origin);
                    res.content_origin = true;
                }
            }

            if let Some(content_size) = geometry.content_size {
                self.set_content_size(content_size);
                res.content_size = true;
            }

            if let Some(size) = geometry.min_frame_size {
                self.set_min_frame_size(size);
                res.min_frame_size = true;
            }

            if let Some(size) = geometry.max_frame_size {
                self.set_max_frame_size(size);
                res.max_frame_size = true;
            }

            if let Some(size) = geometry.min_content_size {
                self.set_min_content_size(size);
                res.min_content_size = true;
            }

            if let Some(size) = geometry.max_content_size {
                self.set_max_content_size(size);
                res.max_content_size = true;
            }

            Ok(res)
        })
    }

    pub fn get_geometry(&self) -> PlatformResult<WindowGeometry> {
        autoreleasepool(|| unsafe {
            Ok(WindowGeometry {
                frame_origin: Some(self.get_frame_origin()),
                frame_size: Some(self.get_frame_size()),
                content_origin: Some(self.get_content_position()),
                content_size: Some(self.get_content_size()),
                min_frame_size: Some(self.get_min_frame_size()),
                max_frame_size: Some(self.get_max_frame_size()),
                min_content_size: Some(self.get_min_content_size()),
                max_content_size: Some(self.get_max_content_size()),
            })
        })
    }

    pub fn supported_geometry(&self) -> PlatformResult<WindowGeometryFlags> {
        let modal = self.is_modal();
        // MacOS supports everything, but when modal (sheet) position is handled by system
        Ok(WindowGeometryFlags {
            frame_origin: !modal,
            frame_size: true,
            content_origin: !modal,
            content_size: true,
            min_frame_size: true,
            max_frame_size: true,
            min_content_size: true,
            max_content_size: true,
        })
    }

    unsafe fn set_frame_origin(&self, position: Point) {
        let screen_frame = NSScreen::frame(self.platform_window.screen());
        let position = Point {
            x: position.x,
            y: screen_frame.size.height - position.y,
        };
        self.platform_window.setFrameTopLeftPoint_(position.into());
    }

    unsafe fn get_frame_origin(&self) -> Point {
        let screen_frame = NSScreen::frame(self.platform_window.screen());
        let window_frame = NSWindow::frame(*self.platform_window);
        Point {
            x: window_frame.origin.x,
            y: screen_frame.size.height - (window_frame.origin.y + window_frame.size.height),
        }
    }

    unsafe fn set_frame_size(&self, size: Size) {
        self.platform_window.setFrameSize(size.into());
    }

    unsafe fn get_frame_size(&self) -> Size {
        NSWindow::frame(*self.platform_window).size.into()
    }

    unsafe fn set_content_position(&self, position: Point) {
        let screen_frame = NSScreen::frame(self.platform_window.screen());
        let content_size = NSView::frame(self.platform_window.contentView()).size;
        let content_rect = NSRect::new(
            Point {
                x: position.x,
                y: screen_frame.size.height - (position.y + content_size.height),
            }
            .into(),
            content_size,
        );
        let window_frame = self.platform_window.frameRectForContentRect_(content_rect);
        self.platform_window.setFrame_display_(window_frame, YES);
    }

    unsafe fn get_content_position(&self) -> Point {
        let screen_frame = NSScreen::frame(self.platform_window.screen());
        let window_frame = NSWindow::frame(*self.platform_window);
        let content_rect = self.platform_window.contentRectForFrameRect_(window_frame);
        Point {
            x: content_rect.origin.x,
            y: screen_frame.size.height - (content_rect.origin.y + content_rect.size.height),
        }
    }

    unsafe fn set_content_size(&self, size: Size) {
        self.platform_window.setContentSize_(size.into());
    }

    pub(super) unsafe fn get_content_size(&self) -> Size {
        NSView::frame(self.platform_window.contentView())
            .size
            .into()
    }

    unsafe fn set_min_frame_size(&self, size: Size) {
        self.platform_window.setMinSize_(size.into());
    }

    unsafe fn get_min_frame_size(&self) -> Size {
        self.platform_window.minSize().into()
    }

    unsafe fn set_max_frame_size(&self, size: Size) {
        self.platform_window.setMaxSize_(size.into());
    }

    unsafe fn get_max_frame_size(&self) -> Size {
        self.platform_window.maxSize().into()
    }

    unsafe fn set_min_content_size(&self, size: Size) {
        self.platform_window.setContentMinSize_(size.into());
    }

    unsafe fn get_min_content_size(&self) -> Size {
        self.platform_window.contentMinSize().into()
    }

    unsafe fn set_max_content_size(&self, size: Size) {
        self.platform_window.setContentMaxSize_(size.into());
    }

    unsafe fn get_max_content_size(&self) -> Size {
        self.platform_window.contentMaxSize().into()
    }

    pub fn perform_window_drag(&self) -> PlatformResult<()> {
        unsafe {
            let last_event = self
                .last_event
                .borrow()
                .values()
                .filter(|e| {
                    let event_type = e.eventType();
                    event_type == NSLeftMouseDown
                })
                .max_by_key(|x| x.eventNumber())
                .cloned();
            if let Some(last_event) = last_event {
                let () = msg_send![*self.platform_window, performWindowDragWithEvent:*last_event];
                Ok(())
            } else {
                Err(PlatformError::NoEventFound)
            }
        }
    }

    pub fn set_style(&self, style: WindowStyle) -> PlatformResult<()> {
        unsafe {
            let mut mask: NSWindowStyleMask = NSWindowStyleMask::NSBorderlessWindowMask;

            if style.frame == WindowFrame::Regular {
                NSWindow::setMovable_(*self.platform_window, YES);
                NSWindow::setTitlebarAppearsTransparent_(*self.platform_window, NO);
            } else {
                NSWindow::setMovable_(*self.platform_window, NO);
                NSWindow::setTitlebarAppearsTransparent_(*self.platform_window, YES);
            }

            if style.frame == WindowFrame::NoTitle {
                mask |= NSWindowStyleMask::NSFullSizeContentViewWindowMask;
            }

            if style.frame != WindowFrame::NoFrame {
                mask |= NSWindowStyleMask::NSTitledWindowMask;
                if style.can_close {
                    mask |= NSWindowStyleMask::NSClosableWindowMask;
                }
                if style.can_resize {
                    mask |= NSWindowStyleMask::NSResizableWindowMask;
                }
                if style.can_minimize {
                    mask |= NSWindowStyleMask::NSMiniaturizableWindowMask;
                }
                NSWindow::setHasShadow_(*self.platform_window, YES);
            } else {
                NSWindow::setHasShadow_(*self.platform_window, NO);
            }

            let mut collection_behavior = NSWindow::collectionBehavior(*self.platform_window);
            let no_fullscreen: NSWindowCollectionBehavior =
                std::mem::transmute((1 << 9) as NSUInteger);
            if !style.can_full_screen {
                collection_behavior |= no_fullscreen;
            } else {
                collection_behavior &= !no_fullscreen;
            }
            NSWindow::setCollectionBehavior_(*self.platform_window, collection_behavior);

            NSWindow::setStyleMask_(*self.platform_window, mask);
        }
        Ok(())
    }

    pub fn set_title(&self, title: String) -> PlatformResult<()> {
        unsafe {
            NSWindow::setTitle_(*self.platform_window, *to_nsstring(&title));
        }
        Ok(())
    }

    pub fn is_modal(&self) -> bool {
        self.modal_close_callback.borrow().is_some()
    }

    unsafe fn actually_show(&self) {
        if self.is_modal() {
            let parent = self.parent_platform_window.as_ref().unwrap().clone().load();
            let () = msg_send![*parent, beginSheet:*self.platform_window completionHandler:nil];
        } else {
            self.platform_window.makeKeyAndOrderFront_(nil);
        }
    }

    fn show_when_ready(weak_self: Weak<PlatformWindow>) {
        if let Some(s) = weak_self.upgrade() {
            autoreleasepool(|| unsafe {
                let layer = NSWindow::contentView(*s.platform_window).layer();
                let sublayers: id = msg_send![layer, sublayers];
                let first = sublayers.objectAtIndex(0);
                let contents: id = msg_send![first, contents];
                if contents != nil {
                    // This makes assumptions about FlutterView internals :-/
                    let class: id = msg_send![contents, className];
                    if !class.isEqualToString("IOSurface") {
                        panic!("Expected IOSurface content");
                    }
                    let scale = NSWindow::backingScaleFactor(*s.platform_window);
                    let content_size = NSView::frame(NSWindow::contentView(*s.platform_window));

                    let expected_width = scale * content_size.size.width;
                    let expected_height = scale * content_size.size.height;
                    // IOSurface width/height
                    let actual_width: NSInteger = msg_send![contents, width];
                    let actual_height: NSInteger = msg_send![contents, height];

                    // only show if size matches, otherwise we caught the view during resizing
                    if actual_width == expected_width as NSInteger
                        && actual_height == expected_height as NSInteger
                    {
                        s.actually_show();
                        if let Some(delegate) = s.delegate.upgrade() {
                            delegate.visibility_changed(true);
                        };
                        return;
                    }
                }
                // wait until we have content generated (with proper size)
                s.context
                    .run_loop
                    .borrow()
                    .schedule(Duration::from_secs_f64(1.0 / 60.0), move || {
                        Self::show_when_ready(weak_self)
                    })
                    .detach();
            })
        }
    }

    pub fn ready_to_show(&self) -> PlatformResult<()> {
        self.ready_to_show.set(true);
        if self.show_when_ready.get() {
            Self::show_when_ready(self.weak_self.clone_value());
        }
        Ok(())
    }

    pub fn show(&self) -> PlatformResult<()> {
        if self.ready_to_show.get() {
            Self::show_when_ready(self.weak_self.clone_value());
        } else {
            self.show_when_ready.set(true);
        }
        Ok(())
    }

    pub fn show_modal<F>(&self, done_callback: F)
    where
        F: FnOnce(PlatformResult<Value>) + 'static,
    {
        self.modal_close_callback
            .borrow_mut()
            .replace(Box::new(done_callback));
        self.show().ok_log();
    }

    pub fn close_with_result(&self, result: Value) -> PlatformResult<()> {
        let callback = self.modal_close_callback.borrow_mut().take();
        if let Some(callback) = callback {
            callback(Ok(result));
        }
        self.close()
    }

    pub fn close(&self) -> PlatformResult<()> {
        autoreleasepool(|| unsafe {
            let sheet_parent: id = msg_send![*self.platform_window, sheetParent];
            if sheet_parent != nil {
                let () = msg_send![sheet_parent, endSheet:*self.platform_window];
            }
            self.platform_window.close();
        });
        Ok(())
    }

    pub fn hide(&self) -> PlatformResult<()> {
        if self.ready_to_show.get() {
            autoreleasepool(|| unsafe {
                self.platform_window.orderOut_(nil);
            });
            if let Some(delegate) = self.delegate.upgrade() {
                delegate.visibility_changed(false);
            }
        } else {
            self.show_when_ready.set(false);
        }
        Ok(())
    }

    unsafe fn synthetize_mouse_up_event(&self) {
        let last_event = self
            .last_event
            .borrow()
            .values()
            .filter(|e| {
                let event_type = e.eventType();
                event_type == NSLeftMouseDown
                    || event_type == NSLeftMouseUp
                    || event_type == NSRightMouseDown
                    || event_type == NSRightMouseUp
            })
            .max_by_key(|x| x.eventNumber())
            .cloned();

        if let Some(event) = last_event {
            let opposite = match event.eventType() {
                NSLeftMouseDown => NSLeftMouseUp,
                NSRightMouseDown => NSRightMouseUp,
                _ => return,
            };

            let opposite: id = msg_send![class!(NSEvent), mouseEventWithType: opposite
                location:NSEvent::mouseLocation(*event)
                modifierFlags:NSEvent::modifierFlags(*event)
                timestamp:NSEvent::timestamp(*event)
                windowNumber:NSEvent::windowNumber(*event)
                context:NSEvent::context(*event)
                eventNumber:event.eventNumber()
                clickCount:1
                pressure:1
            ];

            let () = msg_send![*self.platform_window, sendEvent: opposite];
        }
    }

    pub(super) fn synthetize_mouse_move_if_needed(&self) {
        autoreleasepool(|| unsafe {
            let last_event = self
                .last_event
                .borrow()
                .values()
                .filter(|e| {
                    let event_type = e.eventType();
                    event_type as i32 >= NSLeftMouseDown as i32
                        && event_type as i32 <= NSMouseExited as i32
                })
                .max_by_key(|x| x.eventNumber())
                .cloned();

            if let Some(last_event) = last_event {
                let location = NSEvent::mouseLocation(nil);
                let window_frame = NSWindow::frame(*self.platform_window);
                let content_rect = self.platform_window.contentRectForFrameRect_(window_frame);
                let tail = NSPoint {
                    x: content_rect.origin.x + content_rect.size.width,
                    y: content_rect.origin.y + content_rect.size.height,
                };
                if location.x > content_rect.origin.x
                    && location.x < tail.x
                    && location.y > content_rect.origin.y
                    && location.y < tail.y
                {
                    let location: NSPoint =
                        msg_send![*self.platform_window, convertPointFromScreen: location];
                    let event: id = msg_send![class!(NSEvent), mouseEventWithType: NSMouseMoved
                        location:location
                        modifierFlags:NSEvent::modifierFlags(nil)
                        timestamp: Self::system_uptime()
                        windowNumber:0
                        context:nil
                        eventNumber:NSEvent::eventNumber(*last_event)
                        clickCount:1
                        pressure:0
                    ];
                    let () = msg_send![*self.platform_window, sendEvent: event];
                }
            }
        });
    }

    pub fn should_send_event(&self, event: StrongPtr) -> bool {
        let event_type = unsafe { NSEvent::eventType(*event) };
        if event_type == NSMouseEntered || event_type == NSMouseExited {
            let timestamp = unsafe { NSEvent::timestamp(*event) };
            // we attempt to ignore the event, unfortunately this doesn't work for
            // MouseEntered, as it is delivered to NSTrackingArea by NSApplication
            // directlly. We do however counteract it with
            // subsequent MouseMove produced by synthetize_mouse_move_if_needed()
            if timestamp < self.ignore_enter_leave_until.get() {
                self.synthetize_mouse_move_if_needed();
                return false;
            }
        }
        true
    }

    pub fn set_pending_effect(&self, effect: DragEffect) {
        self.drag_context.borrow_mut().set_pending_effect(effect);
    }

    pub fn begin_drag_session(&self, request: DragRequest) -> PlatformResult<()> {
        let last_down_event = self
            .last_event
            .borrow()
            .get(&(NSLeftMouseDown as u64))
            .cloned();
        if let Some(last_down_event) = last_down_event {
            autoreleasepool(|| unsafe {
                self.drag_context.borrow().start_drag(
                    request,
                    self.platform_window.contentView(),
                    *self.platform_window,
                    *last_down_event,
                );

                self.synthetize_mouse_up_event();
            });
            Ok(())
        } else {
            Err(PlatformError::NoEventFound)
        }
    }

    fn system_uptime() -> f64 {
        unsafe {
            let info = NSProcessInfo::processInfo(nil);
            msg_send![info, systemUptime]
        }
    }

    pub fn show_popup_menu<F>(&self, menu: Rc<PlatformMenu>, request: PopupMenuRequest, on_done: F)
    where
        F: FnOnce(PlatformResult<PopupMenuResponse>) + 'static,
    {
        unsafe {
            // cocoa eats mouse up on popup menu
            self.synthetize_mouse_up_event();

            let position: NSPoint = request.position.into();

            let view = StrongPtr::retain(self.platform_window.contentView());
            let menu = menu.menu.clone();
            let on_done = RefCell::new(Some(Box::new(on_done)));
            let weak = self.weak_self.clone_value();
            let cb = move || {
                let item_selected: BOOL = msg_send![*menu, popUpMenuPositioningItem:nil atLocation:position inView:view.clone()];

                let on_done = on_done.take();
                if let Some(s) = weak.upgrade() {
                    // When hiding menu NSApplication will for whatever reason replay
                    // 'queued' stale MouseEnter/Leave events.
                    s.ignore_enter_leave_until.replace(Self::system_uptime());
                    s.synthetize_mouse_move_if_needed();
                }
                if let Some(on_done) = on_done {
                    on_done(Ok(PopupMenuResponse {
                        item_selected: item_selected == YES,
                    }));
                }
            };
            // this method is likely being invoked from dispatch_async through flutter
            // platform task executor; Showing the popup menu from dispatch_async will block
            // the dispatch queue; Instead we schedule this on next run loop turn, which
            // doesn't block the dispatch queue;
            //
            // Note there is a special support in MacOS PlatformRunLoop when scheduling tasks
            // run in 0 time to not block the dispatch queue.
            self.context.run_loop.borrow().schedule_now(cb).detach();
        }
    }

    pub fn hide_popup_menu(&self, menu: Rc<PlatformMenu>) -> PlatformResult<()> {
        unsafe {
            let () = msg_send![*menu.menu, cancelTracking];
        }
        Ok(())
    }

    pub fn show_system_menu(&self) -> PlatformResult<()> {
        // no system menu in mac
        Err(PlatformError::NotAvailable)
    }

    pub fn set_window_menu(&self, menu: Option<Rc<PlatformMenu>>) -> PlatformResult<()> {
        self.context
            .menu_manager
            .borrow()
            .get_platform_menu_manager()
            .set_menu_for_window(self.platform_window.clone(), menu);
        Ok(())
    }

    pub(super) fn with_delegate<F>(&self, callback: F)
    where
        F: FnOnce(Rc<dyn PlatformWindowDelegate>),
    {
        let delegate = self.delegate.upgrade();
        if let Some(delegate) = delegate {
            callback(delegate);
        }
    }
}

struct WindowClass(*const Class);
unsafe impl Sync for WindowClass {}
struct WindowDelegateClass(*const Class);
unsafe impl Sync for WindowDelegateClass {}

extern "C" fn accepts_first_mouse(_this: &Object, _sel: Sel, _event: id) -> BOOL {
    YES
}

lazy_static! {
    static ref WINDOW_CLASS: WindowClass = unsafe {
        // FlutterView doesn't override acceptsFirstMouse: so we do it here
        {
            let mut class = class_decl_from_name("FlutterView");
            class.add_method(
                sel!(acceptsFirstMouse:),
                accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> BOOL,
            );
        }

        let window_superclass = class!(NSWindow);
        let mut decl = ClassDecl::new("IMFlutterWindow", window_superclass).unwrap();

        decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&Object, Sel));
        decl.add_method(
            sel!(sendEvent:),
            send_event as extern "C" fn(&mut Object, Sel, id),
        );

        decl.add_method(
            sel!(draggingEntered:),
            dragging_entered as extern "C" fn(&mut Object, Sel, id) -> NSDragOperation,
        );

        decl.add_method(
            sel!(draggingUpdated:),
            dragging_updated as extern "C" fn(&mut Object, Sel, id) -> NSDragOperation,
        );

        decl.add_method(
            sel!(draggingExited:),
            dragging_exited as extern "C" fn(&mut Object, Sel, id),
        );

        decl.add_method(
            sel!(performDragOperation:),
            perform_drag_operation as extern "C" fn(&mut Object, Sel, id) -> BOOL,
        );

        decl.add_method(
            sel!(draggingSession:sourceOperationMaskForDraggingContext:),
            source_operation_mask_for_dragging_context
                as extern "C" fn(&mut Object, Sel, id, NSInteger) -> NSDragOperation,
        );

        decl.add_method(
            sel!(draggingSession:endedAtPoint:operation:),
            dragging_session_ended_at_point
                as extern "C" fn(&mut Object, Sel, id, NSPoint, NSDragOperation),
        );

        decl.add_ivar::<*mut c_void>("imState");

        WindowClass(decl.register())
    };
    static ref WINDOW_DELEGATE_CLASS: WindowDelegateClass = unsafe {
        let delegate_superclass = class!(NSResponder);
        let mut decl = ClassDecl::new("IMFlutterWindowDelegate", delegate_superclass).unwrap();

        decl.add_method(
            sel!(windowDidMove:),
            window_did_move as extern "C" fn(&Object, Sel, id),
        );

        decl.add_method(
            sel!(windowShouldClose:),
            window_should_close as extern "C" fn(&Object, Sel, id) -> BOOL,
        );

        decl.add_method(
            sel!(windowWillClose:),
            window_will_close as extern "C" fn(&Object, Sel, id),
        );

        decl.add_method(
            sel!(windowDidBecomeKey:),
            window_did_become_key as extern "C" fn(&Object, Sel, id),
        );

        decl.add_method(
            sel!(windowDidResignKey:),
            window_did_resign_key as extern "C" fn(&Object, Sel, id),
        );

        decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&Object, Sel));

        decl.add_ivar::<*mut c_void>("imState");

        WindowDelegateClass(decl.register())
    };
}

fn with_state<F>(this: &Object, callback: F)
where
    F: FnOnce(Rc<PlatformWindow>),
{
    let state_ptr = unsafe {
        let state_ptr: *mut c_void = *this.get_ivar("imState");
        &mut *(state_ptr as *mut Weak<PlatformWindow>)
    };
    let upgraded = state_ptr.upgrade();
    if let Some(upgraded) = upgraded {
        callback(upgraded);
    }
}

fn with_state_res<F, FR, R>(this: &Object, callback: F, default: FR) -> R
where
    F: FnOnce(Rc<PlatformWindow>) -> R,
    FR: FnOnce() -> R,
{
    let state_ptr = unsafe {
        let state_ptr: *mut c_void = *this.get_ivar("imState");
        &mut *(state_ptr as *mut Weak<PlatformWindow>)
    };
    let upgraded = state_ptr.upgrade();
    if let Some(upgraded) = upgraded {
        callback(upgraded)
    } else {
        default()
    }
}

fn with_state_delegate<F>(this: &Object, callback: F)
where
    F: FnOnce(Rc<PlatformWindow>, Rc<dyn PlatformWindowDelegate>),
{
    with_state(this, move |state| {
        let delegate = state.delegate.upgrade();
        if let Some(delegate) = delegate {
            callback(state, delegate);
        }
    });
}

extern "C" fn window_did_move(this: &Object, _: Sel, _: id) {
    with_state_delegate(this, |_state, _delegate| {});
}

extern "C" fn window_should_close(this: &Object, _: Sel, _: id) -> BOOL {
    with_state_delegate(this, |_state, delegate| {
        delegate.did_request_close();
    });
    NO
}

extern "C" fn window_will_close(this: &Object, _: Sel, _: id) {
    with_state_delegate(this, |state, delegate| {
        unsafe {
            let child_windows: id = msg_send![*state.platform_window, childWindows];
            for i in 0..child_windows.count() {
                child_windows.objectAtIndex(i).close();
            }
            let () = msg_send![*state.platform_window, setContentViewController: nil];
        }
        state
            .context
            .menu_manager
            .borrow()
            .get_platform_menu_manager()
            .window_will_close(state.platform_window.clone());
        delegate.will_close();
    });
}

extern "C" fn window_did_become_key(this: &Object, _: Sel, _: id) {
    with_state_delegate(this, |state, _delegate| {
        state
            .context
            .menu_manager
            .borrow()
            .get_platform_menu_manager()
            .window_did_become_active(state.platform_window.clone());
    });
}

extern "C" fn window_did_resign_key(this: &Object, _: Sel, _: id) {
    with_state_delegate(this, |state, _delegate| {
        state
            .context
            .menu_manager
            .borrow()
            .get_platform_menu_manager()
            .window_did_resign_active(state.platform_window.clone());
    });
}

extern "C" fn send_event(this: &mut Object, _: Sel, e: id) {
    unsafe {
        let event = StrongPtr::retain(e);
        let should_send = with_state_res(
            this,
            move |state| {
                let event_type = NSEvent::eventType(*event);
                state
                    .last_event
                    .borrow_mut()
                    .insert(event_type as u64, event.clone());
                state.should_send_event(event)
            },
            || true,
        );
        if should_send {
            let superclass = superclass(this);
            let () = msg_send![super(this, superclass), sendEvent: e];
        }
    }
}

extern "C" fn dragging_entered(this: &mut Object, _: Sel, info: id) -> NSDragOperation {
    with_state_res(
        this,
        move |state| state.drag_context.borrow().dragging_entered(info),
        || 0,
    )
}

extern "C" fn dragging_updated(this: &mut Object, _: Sel, info: id) -> NSDragOperation {
    with_state_res(
        this,
        move |state| state.drag_context.borrow().dragging_updated(info),
        || 0,
    )
}

extern "C" fn dragging_exited(this: &mut Object, _: Sel, info: id) {
    with_state(this, move |state| {
        state.drag_context.borrow().dragging_exited(info)
    })
}

extern "C" fn perform_drag_operation(this: &mut Object, _: Sel, info: id) -> BOOL {
    with_state_res(
        this,
        move |state| state.drag_context.borrow().perform_drag_operation(info),
        || NO,
    )
}

extern "C" fn source_operation_mask_for_dragging_context(
    this: &mut Object,
    _: Sel,
    session: id,
    context: NSInteger,
) -> NSDragOperation {
    with_state_res(
        this,
        move |state| {
            state
                .drag_context
                .borrow()
                .source_operation_mask_for_dragging_context(session, context)
        },
        || 0,
    )
}

extern "C" fn dragging_session_ended_at_point(
    this: &mut Object,
    _: Sel,
    session: id,
    point: NSPoint,
    operation: NSDragOperation,
) {
    with_state(this, move |state| {
        state
            .drag_context
            .borrow()
            .drag_ended(session, point, operation)
    })
}

extern "C" fn dealloc(this: &Object, _sel: Sel) {
    let state_ptr = unsafe {
        let state_ptr: *mut c_void = *this.get_ivar("imState");
        &mut *(state_ptr as *mut Weak<PlatformWindow>)
    };
    unsafe {
        Box::from_raw(state_ptr);

        let superclass = superclass(this);
        let () = msg_send![super(this, superclass), dealloc];
    }
}

#pragma once
#import <IOKit/pwr_mgt/IOPMLib.h>
#include <stdexcept>

namespace pab_mac {
// ScreenCaptureKit can stop a locked session when its display powers down.
// Wake once and inhibit idle display sleep only while this capture is alive.
// These process-owned assertions do not unlock the session or change pmset.
class CaptureDisplayActivity {
    IOPMAssertionID display_ = kIOPMNullAssertionID;
    IOPMAssertionID wake_ = kIOPMNullAssertionID;
public:
    CaptureDisplayActivity() {
        if (IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep,
                kIOPMAssertionLevelOn, CFSTR("Pixels remote desktop capture"), &display_) != kIOReturnSuccess)
            throw std::runtime_error("Cannot prevent capture display idle sleep");
        if (IOPMAssertionDeclareUserActivity(CFSTR("Pixels remote desktop capture"),
                kIOPMUserActiveLocal, &wake_) != kIOReturnSuccess) {
            IOPMAssertionRelease(display_);
            display_ = kIOPMNullAssertionID;
            throw std::runtime_error("Cannot wake capture display");
        }
    }
    ~CaptureDisplayActivity() {
        if (wake_ != kIOPMNullAssertionID) IOPMAssertionRelease(wake_);
        if (display_ != kIOPMNullAssertionID) IOPMAssertionRelease(display_);
    }
    CaptureDisplayActivity(const CaptureDisplayActivity&) = delete;
    CaptureDisplayActivity& operator=(const CaptureDisplayActivity&) = delete;
};
}

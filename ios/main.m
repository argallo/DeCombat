// The iPad app's entry: hands the process to the Rust client (crates/dc-ios-app -> dc_client::ios_main), which
// creates the UIKit application and its one event loop through winit. Nothing else lives here.
#import <Foundation/Foundation.h>

extern int dc_ios_main(void);

int main(int argc, char *argv[]) {
    (void)argc;
    (void)argv;
    return dc_ios_main();
}

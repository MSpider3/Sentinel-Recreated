#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlinkState {
    Open,
    Closing,
    Closed,
    Opening,
}

pub struct BlinkDetector {
    pub state: BlinkState,
    pub closed_frames: usize,
    pub blink_count: usize,
    pub ear_open_threshold: f32,   // 0.24
    pub ear_closed_threshold: f32, // 0.19
    pub min_blink_duration: usize, // 2
}

impl BlinkDetector {
    pub fn new() -> Self {
        Self {
            state: BlinkState::Open,
            closed_frames: 0,
            blink_count: 0,
            ear_open_threshold: 0.24,
            ear_closed_threshold: 0.19,
            min_blink_duration: 2,
        }
    }

    pub fn update(&mut self, ear: f32) -> bool {
        match self.state {
            BlinkState::Open => {
                if ear < self.ear_closed_threshold {
                    self.state = BlinkState::Closing;
                    self.closed_frames = 1;
                }
            }
            BlinkState::Closing => {
                if ear < self.ear_closed_threshold {
                    self.closed_frames += 1;
                    if self.closed_frames >= self.min_blink_duration {
                        self.state = BlinkState::Closed;
                    }
                } else {
                    // Aborted closing phase
                    self.state = BlinkState::Open;
                    self.closed_frames = 0;
                }
            }
            BlinkState::Closed => {
                if ear > self.ear_open_threshold {
                    self.state = BlinkState::Opening;
                }
            }
            BlinkState::Opening => {
                self.state = BlinkState::Open;
                self.closed_frames = 0;
                self.blink_count += 1;
                return true;
            }
        }
        false
    }
}

impl Default for BlinkDetector {
    fn default() -> Self {
        Self::new()
    }
}

pub fn compute_ear(eye_pts: &[(f32, f32); 6]) -> f32 {
    // eye_pts: [p1, p2, p3, p4, p5, p6]
    let dist = |a: (f32, f32), b: (f32, f32)| -> f32 {
        let dx = a.0 - b.0;
        let dy = a.1 - b.1;
        (dx * dx + dy * dy).sqrt()
    };

    let p2_p6 = dist(eye_pts[1], eye_pts[5]);
    let p3_p5 = dist(eye_pts[2], eye_pts[4]);
    let p1_p4 = dist(eye_pts[0], eye_pts[3]);

    if p1_p4 < 1e-6 {
        return 0.0;
    }
    (p2_p6 + p3_p5) / (2.0 * p1_p4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadPoseChallenge {
    TurnLeft,
    TurnRight,
    TiltUp,
    TiltDown,
}

pub struct HeadPoseDetector;

impl HeadPoseDetector {
    pub fn new() -> Self {
        Self
    }

    pub fn check_challenge(&self, landmarks: &[[f32; 2]; 5], challenge: HeadPoseChallenge) -> bool {
        let left_eye = landmarks[0];
        let right_eye = landmarks[1];
        let nose = landmarks[2];

        let eye_center_x = (left_eye[0] + right_eye[0]) / 2.0;
        let eye_center_y = (left_eye[1] + right_eye[1]) / 2.0;
        let eye_dist = ((left_eye[0] - right_eye[0]).powi(2) + (left_eye[1] - right_eye[1]).powi(2)).sqrt();

        if eye_dist < 1e-5 {
            return false;
        }

        let yaw_ratio = (nose[0] - eye_center_x) / eye_dist;
        let pitch_ratio = (nose[1] - eye_center_y) / eye_dist;

        match challenge {
            HeadPoseChallenge::TurnLeft => yaw_ratio < -0.15,
            HeadPoseChallenge::TurnRight => yaw_ratio > 0.15,
            HeadPoseChallenge::TiltUp => pitch_ratio < 0.25,
            HeadPoseChallenge::TiltDown => pitch_ratio > 0.50,
        }
    }
}

impl Default for HeadPoseDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blink_complete_cycle() {
        let mut detector = BlinkDetector::new();
        // open -> closing -> closed -> opening -> open
        assert!(!detector.update(0.26)); // Open
        assert!(!detector.update(0.18)); // Closing frame 1
        assert!(!detector.update(0.18)); // Closed frame 2
        assert!(!detector.update(0.25)); // Opening
        assert!(detector.update(0.26));  // Open (Blink complete!)
        assert_eq!(detector.blink_count, 1);
    }

    #[test]
    fn test_no_blink_if_not_held_long_enough() {
        let mut detector = BlinkDetector::new();
        assert!(!detector.update(0.26)); // Open
        assert!(!detector.update(0.18)); // Closing frame 1
        assert!(!detector.update(0.25)); // Returned to Open (aborted)
        assert_eq!(detector.blink_count, 0);
    }
}

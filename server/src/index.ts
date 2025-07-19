import express from 'express';
import { setRoutes as setRoutesFS } from './routes/filesystemRoutes';
import { setRoutes as setRoutesAuth } from './routes/authenticationRoutes';
import { AppDataSource } from './data-source';
import { User as FSUser } from './entities/User';
import passport from 'passport';
import session from 'express-session';
import { Strategy as LocalStrategy } from 'passport-local';
import cors from 'cors';
import { AuthenticationController } from './controllers/authenticationController';

const app = express();
const PORT = process.env.PORT || 3000;

app.use(express.json());

app.listen(PORT, () => {
    console.log(`Server is running on http://localhost:${PORT}`);
});

app.use(passport.initialize());

// session in express
app.use(session({
  secret: "shh",
  resave: false,
  saveUninitialized: false
}));
app.use(passport.authenticate('session'));

passport.use(new LocalStrategy(
  async function verify(username: string, password: string, cb) {
    try {
      const user = await new AuthenticationController().getUser(username, password);
      if (!user) {
        return cb(null, false, { message: "Incorrect username or password." });
      }
      return cb(null, user);
    } catch (err) {
      return cb(err);
    }
}));


passport.serializeUser((user: any, done) => {
  done(null, (user as FSUser).username);
});


passport.deserializeUser(async (username: string, done) => {
  try {
    const user = await AppDataSource.getRepository(FSUser).findOneBy({ username });
    done(null, user || false);
  } catch (err) {
    done(err);
  }
});

const corsOptions = {
  origin: 'https://localhost:3000',
  credentials: true,
};
app.use(cors(corsOptions));


// Set up routes
setRoutesFS(app);
setRoutesAuth(app);

// initializing the db
async function db() {
  try {
    await AppDataSource.initialize();
    console.log("Data Source has been initialized and DB schema created.");

    const userRepo = AppDataSource.getRepository(FSUser);
    const exists = await userRepo.findOneBy({ username: "admin" });

    if (!exists) {
      const admin = userRepo.create({
        username: "admin",
        password: "c7be23ada64b3748d4a0aba3604a305535e757f69e5ca67726f013f8303b90fc", // hashed "admin"
        salt: "d610f867285f3cd63aa5ee46e9e1de55"
      });
      await userRepo.save(admin);
      
      // creating the admin (user) folder
      await fetch('http://localhost:3000/api/directories/admin', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ path: '.' }),
      });
    }

  } catch (error) {
    console.error("Error during Data Source initialization: ", error);
  }
}

db();